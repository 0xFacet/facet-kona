#!/usr/bin/env bash

echo "Performance Diagnosis for Kona Validation"
echo "========================================="

# Test 1: Time a single validation
echo -e "\n1. Testing single block validation time..."
time ./bin/validate-facet/validate-facet-fast.sh 10 2>&1 | grep -E "(Successfully|Failed|Error)"

# Test 2: Check RPC latency
echo -e "\n2. Testing RPC endpoint latencies..."
echo -n "L1 RPC: "
time curl -s -X POST ${L1_RPC:?Error: L1_RPC environment variable not set} \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_blockNumber","id":1}' > /dev/null
  
echo -n "L2 RPC: "
time curl -s -X POST ${L2_RPC:?Error: L2_RPC environment variable not set} \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_blockNumber","id":1}' > /dev/null

echo -n "Rollup Node RPC: "
time curl -s -X POST ${ROLLUP_NODE_RPC:?Error: ROLLUP_NODE_RPC environment variable not set} \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"optimism_outputAtBlock","params":["0xa"],"id":1}' > /dev/null

# Test 3: Check if kona-host binary exists
echo -e "\n3. Checking kona-host binary..."
BINARY="./target/release/kona-host"
if [ -f "$BINARY" ]; then
    echo "Binary exists: $(ls -lh $BINARY | awk '{print $5}')"
    echo "Binary date: $(ls -lh $BINARY | awk '{print $6, $7, $8}')"
else
    echo "Binary not found!"
fi

# Test 4: Check disk performance
echo -e "\n4. Testing disk I/O performance..."
echo -n "/tmp write speed: "
dd if=/dev/zero of=/tmp/test.dat bs=1M count=100 2>&1 | grep -oP '\d+(\.\d+)? MB/s'
rm -f /tmp/test.dat

if [ -d "/dev/shm" ]; then
    echo -n "/dev/shm write speed: "
    dd if=/dev/zero of=/dev/shm/test.dat bs=1M count=100 2>&1 | grep -oP '\d+(\.\d+)? MB/s'
    rm -f /dev/shm/test.dat
fi

echo -e "\n5. System resources..."
echo "CPU cores: $(nproc)"
echo "Available memory: $(free -h | grep Mem | awk '{print $7}')"
echo "Load average: $(uptime | awk -F'load average:' '{print $2}')"