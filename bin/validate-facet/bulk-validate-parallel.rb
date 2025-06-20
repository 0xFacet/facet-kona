#!/usr/bin/env ruby

require 'parallel'
require 'json'
require 'fileutils'
require 'time'
require 'optparse'
require 'open3'
require 'timeout'
require 'ostruct'

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
    # Use in_processes for better parallelism with external commands
    parallel_results = Parallel.map(blocks, in_processes: options[:workers], progress: "Validating") do |block|
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
    
    if blocks.empty?
      puts "\n✅ No blocks to process!"
    else
      puts "\n📋 Total blocks to validate: #{blocks.length}"
    end
    
    blocks
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
        
        # Run validation with optimized environment
        env = { 
          "DATA_DIR" => data_dir,
          "RUST_LOG" => "warn",  # Use warn level for now
          "RUST_BACKTRACE" => "0"
        }
        
        # Initialize variables
        stdout = ""
        stderr = ""
        status = nil
        
        # Run with timeout to prevent hanging
        begin
          Timeout::timeout(60) do  # 60 second timeout
            stdout, stderr, status = Open3.capture3(env, "./bin/validate-facet/validate-facet.sh #{block_number}")
          end
        rescue Timeout::Error
          stdout = ""
          stderr = "Validation timed out after 60 seconds"
          # Create a fake status object that behaves like Process::Status
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
  output_dir: nil
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
  
  opts.separator ""
  opts.separator "Output Options:"
  
  opts.on("-o", "--output DIR", "Output directory (default: auto-generated)") do |v|
    options[:output_dir] = v
  end
  
  opts.on("-r", "--resume", "Resume from previous checkpoint") do
    options[:resume] = true
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
end.parse!

# Check dependencies
begin
  require 'parallel'
  rescue LoadError => e
  puts "Missing required gem: #{e.message.split(' ').last}"
  puts "\nPlease install dependencies:"
  puts "  gem install parallel"
  exit 1
end

# Run validator
validator = FacetBulkValidator.new(options)
validator.run