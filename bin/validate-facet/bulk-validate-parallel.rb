#!/usr/bin/env ruby

require 'parallel'
require 'json'
require 'fileutils'
require 'time'
require 'optparse'
require 'open3'
require 'timeout'
require 'ostruct'
require 'set'
require 'httparty'
require 'lru_redux'

class FacetBulkValidator
  attr_reader :options, :output_dir, :results_file, :start_time

  def initialize(options)
    @options = options
    @start_time = Time.now
    
    # Setup output directory
    @output_dir = options[:output_dir] || "validation_#{Time.now.strftime('%Y%m%d_%H%M%S')}"
    FileUtils.mkdir_p(@output_dir)
    FileUtils.mkdir_p(File.join(@output_dir, 'logs'))
    
    @results_file = File.join(@output_dir, 'results.jsonl')
    @checkpoint_file = File.join(@output_dir, 'checkpoint.json')
    @summary_file = File.join(@output_dir, 'summary.json')
    
    # Thread-safe counters for real-time progress
    @success_count = 0
    @failure_count = 0
    @total_count = 0
    @counter_mutex = Mutex.new
    
    # RPC response cache with bounded size (thread-safe LRU)
    # 10,000 entries should handle most validation runs without unbounded growth
    @rpc_cache = LruRedux::ThreadSafeCache.new(1_000_000)
  end

  def run
    print_header
    build_kona_host
    
    blocks = determine_blocks_to_process
    return if blocks.empty?
    
    # Results tracking
    results = []
    failed_blocks = []
    
    puts "\n📋 Processing #{blocks.length} blocks with #{options[:workers]} workers...\n"
    
    # Process blocks in parallel with progress bar
    # Use in_threads for better parallelism with external commands
    parallel_results = Parallel.map(blocks, in_threads: options[:workers], progress: "Validating") do |block|
      validate_block(block)
    end
    
    # Collect results after parallel processing
    parallel_results.each do |result|
      next unless result  # Skip nil results
      
      results << result
      failed_blocks << result[:block] unless result[:success]
      
      # Append to results file
      File.open(@results_file, 'a') { |f| f.puts result.to_json }
    end
    
    # Generate final report
    generate_report(results, failed_blocks)
  end

  private

  def rpc_call(url, method, params, timeout_seconds = 30)
    # Check cache first for certain methods
    cache_key = nil
    if method == 'eth_getBlockByNumber' && params[0].is_a?(String) && params[0].start_with?('0x')
      # Normalize cache key: downcase hex, convert boolean to string
      normalized_hex = params[0].downcase
      normalized_bool = params[1].to_s
      cache_key = "#{url}:#{method}:#{normalized_hex}:#{normalized_bool}"
      
      # ThreadSafeCache handles synchronization internally
      cached_value = @rpc_cache[cache_key]
      return cached_value if cached_value
    end
    
    # Prepare RPC payload
    payload = {
      jsonrpc: '2.0',
      method: method,
      params: params,
      id: 1
    }
    
    # Make HTTP request using HTTParty
    begin
      response = HTTParty.post(
        url,
        body: payload.to_json,
        headers: { 'Content-Type' => 'application/json' },
        timeout: timeout_seconds
      )
      
      if response.code != 200
        raise "HTTP Error: #{response.code} #{response.message}"
      end
      
      result = response.parsed_response
      
      if result['error']
        raise "RPC Error: #{result['error']['message']} (code: #{result['error']['code']})"
      end
      
      # Cache successful responses for certain methods
      if cache_key
        @rpc_cache[cache_key] = result['result']
      end
      
      result['result']
    rescue Net::OpenTimeout, Net::ReadTimeout => e
      raise "RPC Timeout: #{e.message} for #{method} on #{url}"
    rescue HTTParty::Error => e
      raise "HTTParty Error: #{e.message}"
    rescue JSON::ParserError => e
      raise "Invalid JSON response: #{e.message}"
    rescue => e
      raise "RPC Call failed: #{e.class}: #{e.message}"
    end
  end

  def print_header
    puts "🚀 Facet Bulk Validation Tool (Parallel)"
    puts "="*50
    puts "Range: #{options[:start_block]} - #{options[:end_block]}"
    puts "Workers: #{options[:workers]}"
    puts "Sample Rate: 1/#{options[:sample_rate]}"
    puts "Output: #{output_dir}"
    puts "="*50
  end

  def build_kona_host
    print "\n🔨 Building kona-host... "
    if system("cargo build --bin kona-host --release > /dev/null 2>&1")
      puts "✅"
    else
      puts "❌"
      raise "Failed to build kona-host"
    end
  end

  def determine_blocks_to_process
    blocks = (options[:start_block]..options[:end_block]).to_a
    
    # Apply sampling
    if options[:sample_rate] > 1
      original_count = blocks.length
      blocks = blocks.select { |b| b % options[:sample_rate] == 0 }
      puts "\n📊 Sampling: Testing #{blocks.length} of #{original_count} blocks (every #{options[:sample_rate]})"
    end
    
    # Apply random sampling if requested
    if options[:random_sample]
      blocks = blocks.sample(options[:random_sample], random: Random.new(options[:random_seed]))
      puts "🎲 Random sampling: Selected #{blocks.length} blocks (seed: #{options[:random_seed]})"
    end
    
    # Handle resume
    if options[:resume] && File.exist?(@checkpoint_file)
      checkpoint = JSON.parse(File.read(@checkpoint_file))
      processed = checkpoint['processed_blocks'].to_set
      original_count = blocks.length
      blocks = blocks.reject { |b| processed.include?(b) }
      puts "♻️  Resume: Skipping #{processed.size} already processed blocks"
    end
    
    # Handle exclude-success option
    if options[:exclude_success_from] && File.exist?(options[:exclude_success_from])
      successful_blocks = Set.new
      File.foreach(options[:exclude_success_from]) do |line|
        begin
          result = JSON.parse(line)
          successful_blocks << result['block'] if result['success']
        rescue JSON::ParserError
          # Skip invalid lines
        end
      end
      
      original_count = blocks.length
      blocks = blocks.reject { |b| successful_blocks.include?(b) }
      puts "🚫 Excluding #{successful_blocks.size} previously successful blocks from #{options[:exclude_success_from]}"
      puts "   Remaining blocks to validate: #{blocks.length}"
    end
    
    if blocks.empty?
      puts "\n✅ No blocks to process!"
    else
      puts "\n📋 Total blocks to validate: #{blocks.length}"
    end
    
    blocks
  end

  def gather_params(block_number)
    rollup_rpc = ENV.fetch('ROLLUP_NODE_RPC') { raise "ROLLUP_NODE_RPC not set" }
    l1_rpc = ENV.fetch('L1_RPC') { raise "L1_RPC not set" }
    l2_rpc = ENV.fetch('L2_RPC') { raise "L2_RPC not set" }
    
    hex_block = "0x#{block_number.to_s(16)}"
    prev_block = block_number - 1
    hex_prev_block = "0x#{prev_block.to_s(16)}"
    
    threads = {}
    errors = []
    
    # Launch the four requests in parallel threads
    threads[:claimed_l2_output_root] = Thread.new do
      begin
        result = rpc_call(rollup_rpc, 'optimism_outputAtBlock', [hex_block])
        result['outputRoot']
      rescue => e
        errors << "Failed to get claimed output root: #{e.message}"
        nil
      end
    end
    
    threads[:prev_output] = Thread.new do
      begin
        rpc_call(rollup_rpc, 'optimism_outputAtBlock', [hex_prev_block])
      rescue => e
        errors << "Failed to get previous output: #{e.message}"
        nil
      end
    end
    
    threads[:agreed_l2_head_hash] = Thread.new do
      begin
        result = rpc_call(l2_rpc, 'eth_getBlockByNumber', [hex_prev_block, false])
        result['hash']
      rescue => e
        errors << "Failed to get L2 head hash: #{e.message}"
        nil
      end
    end
    
    # Wait for prev_output to finish so we can derive L1 parameters
    prev_output = threads[:prev_output].value
    
    if prev_output.nil?
      # Join all threads and raise aggregate error
      threads.each { |_, t| t.join }
      raise errors.join("\n")
    end
    
    agreed_l2_output_root = prev_output['outputRoot']
    l1_origin_num = prev_output['blockRef']['l1origin']['number']
    l1_origin_num = l1_origin_num.is_a?(String) ? l1_origin_num.to_i(16) : l1_origin_num.to_i
    l1_head_block_num = l1_origin_num + 30
    l1_head_hex = "0x#{l1_head_block_num.to_s(16)}"
    
    threads[:l1_head] = Thread.new do
      begin
        result = rpc_call(l1_rpc, 'eth_getBlockByNumber', [l1_head_hex, false])
        result['hash']
      rescue => e
        errors << "Failed to get L1 head: #{e.message}"
        nil
      end
    end
    
    # Join all threads and collect results
    results = {}
    results[:claimed_l2_output_root] = threads[:claimed_l2_output_root].value
    results[:agreed_l2_head_hash] = threads[:agreed_l2_head_hash].value
    results[:l1_head] = threads[:l1_head].value
    
    # Check for any nil results
    results.each do |key, value|
      if value.nil?
        threads.each { |_, t| t.join if t.alive? }
        raise errors.join("\n")
      end
    end
    
    # Determine rollup config path
    script_dir = File.dirname(File.expand_path(__FILE__))
    default_config = File.join(script_dir, 'facet-mainnet-rollup-config.json')
    rollup_config_path = options[:rollup_config] || ENV.fetch('ROLLUP_CONFIG_PATH', default_config)
    
    {
      claimed_l2_output_root: results[:claimed_l2_output_root],
      agreed_l2_output_root: agreed_l2_output_root,
      agreed_l2_head_hash: results[:agreed_l2_head_hash],
      l1_head: results[:l1_head],
      rollup_config_path: rollup_config_path
    }
  end

  def validate_block(block_number)
    start_time = Time.now
    log_file = File.join(output_dir, 'logs', "block_#{block_number}.log")
    
    # Use fast temp directory (RAM disk if available, otherwise /tmp)
    temp_base = File.exist?("/dev/shm") ? "/dev/shm" : "/tmp"
    data_dir = "#{temp_base}/kona_data_#{Process.pid}_block_#{block_number}"
    
    result = {
      block: block_number,
      timestamp: Time.now.iso8601,
      success: false,
      error: nil,
      duration_ms: 0,
      output_root: nil,
      retries: 0
    }
    
    output = nil  # Define output variable outside the retry loop
    
    begin
      # Retry logic
      (0..options[:max_retries]).each do |retry_count|
        result[:retries] = retry_count
        
        # Initialize variables
        stdout = ""
        stderr = ""
        status = nil
        
        begin
          # Gather parameters via RPC calls
          params = nil
          begin
            params = gather_params(block_number)
          rescue => e
            stdout = ""
            stderr = "Failed to gather parameters: #{e.message}"
            # Skip to error handling below
          end
          
          if params
            # Run validation with optimized environment
            env = { 
              "DATA_DIR" => data_dir,
              "RUST_LOG" => ENV.fetch("RUST_LOG", "warn"),
              "RUST_BACKTRACE" => "0",
              # Pass through RPC environment variables
              "L1_RPC" => ENV["L1_RPC"],
              "L1_BEACON_RPC" => ENV["L1_BEACON_RPC"],
              "L2_RPC" => ENV["L2_RPC"],
              "ROLLUP_NODE_RPC" => ENV["ROLLUP_NODE_RPC"],
              "L1_NETWORK" => ENV["L1_NETWORK"]
            }.compact  # Remove nil values
            
            # Build kona-host command
            cmd = [
              "./target/release/kona-host",
              "-vvv", "single",
              "--l1-head", params[:l1_head],
              "--agreed-l2-head-hash", params[:agreed_l2_head_hash],
              "--claimed-l2-output-root", params[:claimed_l2_output_root],
              "--agreed-l2-output-root", params[:agreed_l2_output_root],
              "--claimed-l2-block-number", block_number.to_s,
              "--rollup-config-path", params[:rollup_config_path],
              "--l1-node-address", ENV["L1_RPC"],
              "--l1-beacon-address", ENV["L1_BEACON_RPC"],
              "--l2-node-address", ENV["L2_RPC"],
              "--native",
              "--data-dir", data_dir
            ]
            
            # Run with proper timeout handling using process management
            stdin, stdout_io, stderr_io, wait_thr = Open3.popen3(env, *cmd, pgroup: true)
            stdin.close
            
            start_time = Time.now
            timeout_seconds = options[:timeout]
            status = nil
            
            # Poll for process completion
            while (Time.now - start_time) < timeout_seconds
              if wait_thr.join(1)  # Wait up to 1 second
                status = wait_thr.value
                break
              end
            end
            
            # Handle timeout
            if status.nil?
              begin
                # Send TERM signal to process group
                Process.kill('-TERM', wait_thr.pid)
                
                # Give it 5 seconds to terminate gracefully
                if wait_thr.join(5)
                  status = wait_thr.value
                else
                  # Force kill if still running
                  Process.kill('-KILL', wait_thr.pid) rescue nil
                  wait_thr.join  # Wait for it to die
                  status = wait_thr.value
                end
              rescue Errno::ESRCH
                # Process already dead
                status = wait_thr.value
              end
              
              stdout = stdout_io.read rescue ""
              stderr = "Validation timed out after #{timeout_seconds} seconds\n" + (stderr_io.read rescue "")
            else
              # Process completed normally
              stdout = stdout_io.read rescue ""
              stderr = stderr_io.read rescue ""
            end
            
            # Close IO streams
            [stdout_io, stderr_io].each { |io| io.close rescue nil }
          else
            # params is nil, create fake failed status
            status = Object.new
            def status.success?
              false
            end
          end
        rescue => e
          # Handle any errors during validation attempt
          stdout = ""
          stderr = "Error during validation: #{e.class}: #{e.message}\n#{e.backtrace.first(5).join("\n")}"
          status = Object.new
          def status.success?
            false
          end
        end
        
        # Save combined log
        output = "STDOUT:\n#{stdout}\n\nSTDERR:\n#{stderr}"
        File.write(log_file, output)
        
        # Check if validation succeeded
        if status && status.success? && stdout.include?("Successfully validated L2 block")
          result[:success] = true
          result[:output_root] = extract_output_root(stdout)
          break
        else
          # Extract meaningful error (ignore backtrace hints)
          result[:error] = extract_error(stdout + "\n" + stderr)
          
          # Retry with backoff
          if retry_count < options[:max_retries]
            sleep(2 ** retry_count)
          end
        end
      end
      
      result[:duration_ms] = ((Time.now - start_time) * 1000).to_i
      
      # Print inline status for failures
      unless result[:success]
        puts "\n❌ Block #{block_number} failed after #{result[:retries]} retries (#{result[:duration_ms]}ms)"
        puts "   Error: #{result[:error]}"
        
        # Extract and show key error details from logs
        if output && output.include?("output root mismatch")
          if computed_root = output.match(/computed[=:]?\s*([0-9a-fx]+)/i)
            puts "   Computed: #{computed_root[1]}"
          end
          if expected_root = output.match(/expected[=:]?\s*([0-9a-fx]+)/i)
            puts "   Expected: #{expected_root[1]}"
          end
        end
        
        # Show if it's a specific type of error
        error_lower = result[:error].to_s.downcase
        if error_lower.include?("timeout")
          puts "   Type: Timeout error"
        elsif error_lower.include?("rate limit")
          puts "   Type: Rate limit hit"
        elsif error_lower.include?("connection")
          puts "   Type: Network/connection issue"
        end
      end
      
      # Update counters and show running totals
      @counter_mutex.synchronize do
        @total_count += 1
        if result[:success]
          @success_count += 1
        else
          @failure_count += 1
        end
        
        # Show running totals periodically (every 25 blocks) or on failures
        if @total_count % 25 == 0 || !result[:success]
          success_rate = @total_count > 0 ? (@success_count * 100.0 / @total_count).round(1) : 0
          elapsed = Time.now - @start_time
          rate = @total_count / elapsed * 60
          puts "\n📊 Progress: #{@total_count} processed | #{@success_count} success (#{success_rate}%) | #{@failure_count} failures | #{rate.round(1)} blocks/min\n"
        end
      end
    rescue => e
      # Catch any unexpected exceptions at the method level
      result[:error] = "Fatal error: #{e.class}: #{e.message}"
      result[:duration_ms] = ((Time.now - start_time) * 1000).to_i
      
      # Log the error
      error_log = "FATAL ERROR:\n#{e.class}: #{e.message}\n\nBacktrace:\n#{e.backtrace.join("\n")}"
      File.write(log_file, error_log)
      
      puts "\n💥 Block #{block_number} encountered fatal error: #{e.class}: #{e.message}"
    ensure
      # Cleanup temp directory
      if defined?(data_dir) && data_dir && Dir.exist?(data_dir)
        FileUtils.rm_rf(data_dir)
      end
    end
    
    result
  end

  def extract_output_root(output)
    if match = output.match(/output_root[=:]?\s*([0-9a-fx]+)/i)
      match[1]
    end
  end

  def extract_error(output)
    # Look for specific error patterns
    patterns = [
      /Failed to validate.*?: (.+)/,
      /ERROR\s+\w+:\s+(.+)/,
      /Error: (.+)/
    ]
    
    patterns.each do |pattern|
      if match = output.match(pattern)
        return match[1].strip
      end
    end
    
    # Fallback: find last meaningful line (not backtrace hint)
    meaningful_lines = output.lines
      .map(&:strip)
      .reject(&:empty?)
      .reject { |line| line.include?("RUST_BACKTRACE") }
      .reject { |line| line.start_with?("note:") }
    
    meaningful_lines.last || "Unknown error"
  end

  def generate_report(results, failed_blocks)
    total = results.length
    successful = results.count { |r| r[:success] }
    failed = total - successful
    duration = Time.now - start_time
    
    puts "\n\n" + "="*60
    puts "🏁 VALIDATION COMPLETE"
    puts "="*60
    
    # Summary stats
    stats = {
      "Total Blocks" => total,
      "Successful" => "#{successful} (#{format_percent(successful, total)})",
      "Failed" => "#{failed} (#{format_percent(failed, total)})",
      "Duration" => format_duration(duration),
      "Avg Time/Block" => "#{(duration / total).round(1)}s",
      "Blocks/Minute" => (total * 60.0 / duration).round(1)
    }
    
    stats.each do |label, value|
      puts "#{label.ljust(15)}: #{value}"
    end
    
    # Failed blocks summary
    if failed > 0
      puts "\n❌ Failed Blocks:"
      
      # Group errors
      error_groups = results
        .select { |r| !r[:success] }
        .group_by { |r| r[:error] || "Unknown" }
      
      error_groups.each do |error, blocks|
        puts "\n  #{error}:"
        blocks.each { |b| puts "    - Block #{b[:block]}" }
      end
    else
      puts "\n✅ All blocks validated successfully!"
    end
    
    # Save summary
    summary = {
      configuration: {
        start_block: options[:start_block],
        end_block: options[:end_block],
        workers: options[:workers],
        sample_rate: options[:sample_rate],
        timestamp: Time.now.iso8601
      },
      results: {
        total: total,
        successful: successful,
        failed: failed,
        success_rate: successful * 100.0 / total,
        duration_seconds: duration.round(2),
        blocks_per_minute: (total * 60.0 / duration).round(1)
      },
      failed_blocks: failed_blocks.sort,
      error_summary: results
        .select { |r| !r[:success] }
        .group_by { |r| r[:error] }
        .transform_values(&:count)
    }
    
    File.write(@summary_file, JSON.pretty_generate(summary))
    
    puts "\n📁 Results saved to: #{output_dir}/"
    puts "   - Summary: #{@summary_file}"
    puts "   - Details: #{@results_file}"
    puts "   - Logs: #{output_dir}/logs/"
  end

  def format_percent(count, total)
    return "0%" if total == 0
    "#{(count * 100.0 / total).round(1)}%"
  end

  def format_duration(seconds)
    if seconds < 60
      "#{seconds.round(1)}s"
    elsif seconds < 3600
      minutes = (seconds / 60).to_i
      secs = (seconds % 60).to_i
      "#{minutes}m #{secs}s"
    else
      hours = (seconds / 3600).to_i
      minutes = ((seconds % 3600) / 60).to_i
      "#{hours}h #{minutes}m"
    end
  end

  def save_checkpoint(processed_blocks)
    checkpoint = {
      processed_blocks: processed_blocks,
      timestamp: Time.now.iso8601
    }
    File.write(@checkpoint_file, JSON.pretty_generate(checkpoint))
  end
end

# Parse command line options
options = {
  start_block: 10,
  end_block: 20,
  workers: 4,
  sample_rate: 1,
  max_retries: 10,
  random_sample: nil,
  random_seed: 42,
  resume: false,
  output_dir: nil,
  exclude_success_from: nil,
  rollup_config: nil,
  timeout: 180
}

OptionParser.new do |opts|
  opts.banner = "Usage: #{$0} [options]"
  
  opts.separator ""
  opts.separator "Block Range:"
  
  opts.on("-s", "--start BLOCK", Integer, "Start block number (default: #{options[:start_block]})") do |v|
    options[:start_block] = v
  end
  
  opts.on("-e", "--end BLOCK", Integer, "End block number (default: #{options[:end_block]})") do |v|
    options[:end_block] = v
  end
  
  opts.separator ""
  opts.separator "Execution Options:"
  
  opts.on("-j", "--jobs N", Integer, "Number of parallel workers (default: #{options[:workers]})") do |v|
    options[:workers] = v
  end
  
  opts.on("--sample-rate N", Integer, "Test every Nth block (default: #{options[:sample_rate]})") do |v|
    options[:sample_rate] = v
  end
  
  opts.on("--random N", Integer, "Randomly test N blocks from range") do |v|
    options[:random_sample] = v
  end
  
  opts.on("--seed N", Integer, "Random seed for reproducibility (default: #{options[:random_seed]})") do |v|
    options[:random_seed] = v
  end
  
  opts.on("--retries N", Integer, "Max retries per block (default: #{options[:max_retries]})") do |v|
    options[:max_retries] = v
  end
  
  opts.on("--rollup-config PATH", "Path to rollup config JSON (default: facet-mainnet-rollup-config.json)") do |v|
    options[:rollup_config] = v
  end
  
  opts.on("--timeout SECONDS", Integer, "Timeout for kona-host execution (default: #{options[:timeout]})") do |v|
    options[:timeout] = v
  end
  
  opts.separator ""
  opts.separator "Output Options:"
  
  opts.on("-o", "--output DIR", "Output directory (default: auto-generated)") do |v|
    options[:output_dir] = v
  end
  
  opts.on("-r", "--resume", "Resume from previous checkpoint") do
    options[:resume] = true
  end
  
  opts.on("--exclude-success-from FILE", "Skip blocks that were successful in the specified results.jsonl file") do |v|
    options[:exclude_success_from] = v
  end
  
  opts.separator ""
  opts.separator "Other:"
  
  opts.on("-h", "--help", "Show this help message") do
    puts opts
    exit
  end
  
  opts.separator ""
  opts.separator "Examples:"
  opts.separator "  #{$0} --start 100 --end 200 --jobs 8"
  opts.separator "  #{$0} --start 1 --end 1000 --sample-rate 10"
  opts.separator "  #{$0} --start 1 --end 10000 --random 100 --jobs 16"
  opts.separator "  #{$0} --start 1 --end 1000 --exclude-success-from validation_20250720_122657/results.jsonl"
end.parse!

# Check dependencies
required_gems = ['parallel', 'httparty', 'lru_redux']
missing_gems = []

required_gems.each do |gem_name|
  begin
    require gem_name
  rescue LoadError
    missing_gems << gem_name
  end
end

unless missing_gems.empty?
  puts "Missing required gems: #{missing_gems.join(', ')}"
  puts "\nPlease install dependencies:"
  puts "  gem install #{missing_gems.join(' ')}"
  exit 1
end

# Run validator
validator = FacetBulkValidator.new(options)
validator.run