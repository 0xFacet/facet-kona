#!/bin/bash

# Script to switch between GitHub and local OP Alloy development

usage() {
    echo "Usage: $0 [github|local]"
    echo ""
    echo "Switch between GitHub and local OP Alloy dependencies:"
    echo "  github - Use 0xFacet/facet-op-alloy tag v0.15.4-facet (default)"
    echo "  local  - Use local OP Alloy development path"
    echo ""
    echo "Example:"
    echo "  $0 local    # Switch to local development"
    echo "  $0 github   # Switch back to GitHub"
    exit 1
}

MODE=${1:-github}

if [[ "$MODE" != "github" && "$MODE" != "local" ]]; then
    usage
fi

echo "🔧 Switching OP Alloy dependencies to: $MODE"

# Create backup
cp Cargo.toml Cargo.toml.bak

if [ "$MODE" = "local" ]; then
    echo "📁 Switching to local OP Alloy development..."
    
    # Since OP Alloy paths are already uncommented in Cargo.toml, we don't need to do anything
    echo "✅ Already using local OP Alloy at ../facet-op-alloy"
    echo "⚠️  Make sure your local OP Alloy is at ../facet-op-alloy relative to this directory"
else
    echo "🌐 Switching to GitHub OP Alloy..."
    
    # We need to add git dependencies for OP Alloy to the patch section
    # First, let's add a comment section for GitHub dependencies if it doesn't exist
    
    # Check if we need to add GitHub dependencies section
    if ! grep -q "# op-alloy-network = { git =" Cargo.toml; then
        echo "⚠️  GitHub OP Alloy dependencies not found in Cargo.toml"
        echo "📝 Adding GitHub dependencies configuration..."
        
        # Add GitHub dependencies after the local paths
        sed -i '' '/^op-alloy-rpc-types-engine = { path = "\.\.\/facet-op-alloy\/crates\/rpc-types-engine" }/a\
# For GitHub development, uncomment these lines and comment out the path-based patches above:\
# op-alloy-network = { git = "https://github.com/0xFacet/facet-op-alloy", tag = "v0.15.4-facet" }\
# op-alloy-provider = { git = "https://github.com/0xFacet/facet-op-alloy", tag = "v0.15.4-facet" }\
# op-alloy-consensus = { git = "https://github.com/0xFacet/facet-op-alloy", tag = "v0.15.4-facet" }\
# op-alloy-rpc-types = { git = "https://github.com/0xFacet/facet-op-alloy", tag = "v0.15.4-facet" }\
# op-alloy-rpc-jsonrpsee = { git = "https://github.com/0xFacet/facet-op-alloy", tag = "v0.15.4-facet" }\
# op-alloy-rpc-types-engine = { git = "https://github.com/0xFacet/facet-op-alloy", tag = "v0.15.4-facet" }
' Cargo.toml
    fi
    
    # Comment out local paths and uncomment git dependencies
    sed -i '' '
    /^op-alloy-network = { path = "\.\.\/facet-op-alloy\/crates\/network"/s/^/# /
    /^op-alloy-provider = { path = "\.\.\/facet-op-alloy\/crates\/provider"/s/^/# /
    /^op-alloy-consensus = { path = "\.\.\/facet-op-alloy\/crates\/consensus"/s/^/# /
    /^op-alloy-rpc-types = { path = "\.\.\/facet-op-alloy\/crates\/rpc-types"/s/^/# /
    /^op-alloy-rpc-jsonrpsee = { path = "\.\.\/facet-op-alloy\/crates\/rpc-jsonrpsee"/s/^/# /
    /^op-alloy-rpc-types-engine = { path = "\.\.\/facet-op-alloy\/crates\/rpc-types-engine"/s/^/# /
    ' Cargo.toml
    
    sed -i '' '
    /^# op-alloy-network = { git = "https:\/\/github.com\/0xFacet\/facet-op-alloy"/s/^# //
    /^# op-alloy-provider = { git = "https:\/\/github.com\/0xFacet\/facet-op-alloy"/s/^# //
    /^# op-alloy-consensus = { git = "https:\/\/github.com\/0xFacet\/facet-op-alloy"/s/^# //
    /^# op-alloy-rpc-types = { git = "https:\/\/github.com\/0xFacet\/facet-op-alloy"/s/^# //
    /^# op-alloy-rpc-jsonrpsee = { git = "https:\/\/github.com\/0xFacet\/facet-op-alloy"/s/^# //
    /^# op-alloy-rpc-types-engine = { git = "https:\/\/github.com\/0xFacet\/facet-op-alloy"/s/^# //
    ' Cargo.toml
    
    echo "✅ Switched to GitHub OP Alloy (0xFacet/facet-op-alloy tag v0.15.4-facet)"
fi

echo ""
echo "🔄 Running cargo update to refresh dependencies..."
cargo clean
cargo check -p kona-executor

echo ""
echo "✅ Done! OP Alloy dependencies are now using: $MODE"