#!/bin/bash

# Script to switch between GitHub and local REVM development

usage() {
    echo "Usage: $0 [github|local]"
    echo ""
    echo "Switch between GitHub and local REVM dependencies:"
    echo "  github - Use 0xFacet/facet-revm branch (default)"
    echo "  local  - Use local REVM development path"
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

echo "🔧 Switching REVM dependencies to: $MODE"

# Create backup
cp Cargo.toml Cargo.toml.bak

if [ "$MODE" = "local" ]; then
    echo "📁 Switching to local REVM development..."
    
    # Comment out git dependencies and uncomment local paths
    sed -i '' '
    /^revm = { version = "22.0.1", git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^/# /
    /^revm-bytecode = { git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^/# /
    /^revm-context = { git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^/# /
    /^revm-context-interface = { git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^/# /
    /^revm-database = { git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^/# /
    /^revm-database-interface = { git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^/# /
    /^revm-handler = { git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^/# /
    /^revm-inspector = { git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^/# /
    /^revm-interpreter = { git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^/# /
    /^revm-precompile = { git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^/# /
    /^revm-primitives = { git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^/# /
    /^revm-state = { git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^/# /
    /^op-revm = { version = "3.0.1", git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^/# /
    ' Cargo.toml
    
    sed -i '' '
    /^# revm = { version = "22.0.1", path = "\.\.\/revm\/crates\/revm"/s/^# //
    /^# revm-bytecode = { path = "\.\.\/revm\/crates\/bytecode"/s/^# //
    /^# revm-context = { path = "\.\.\/revm\/crates\/context"/s/^# //
    /^# revm-context-interface = { path = "\.\.\/revm\/crates\/context\/interface"/s/^# //
    /^# revm-database = { path = "\.\.\/revm\/crates\/database"/s/^# //
    /^# revm-database-interface = { path = "\.\.\/revm\/crates\/database\/interface"/s/^# //
    /^# revm-handler = { path = "\.\.\/revm\/crates\/handler"/s/^# //
    /^# revm-inspector = { path = "\.\.\/revm\/crates\/inspector"/s/^# //
    /^# revm-interpreter = { path = "\.\.\/revm\/crates\/interpreter"/s/^# //
    /^# revm-precompile = { path = "\.\.\/revm\/crates\/precompile"/s/^# //
    /^# revm-primitives = { path = "\.\.\/revm\/crates\/primitives"/s/^# //
    /^# revm-state = { path = "\.\.\/revm\/crates\/state"/s/^# //
    /^# op-revm = { version = "3.0.1", path = "\.\.\/revm\/crates\/optimism"/s/^# //
    ' Cargo.toml
    
    echo "✅ Switched to local REVM at ../revm"
    echo "⚠️  Make sure your local REVM is at ../revm relative to this directory"
else
    echo "🌐 Switching to GitHub REVM..."
    
    # Comment out local paths and uncomment git dependencies
    sed -i '' '
    /^revm = { version = "22.0.1", path = "\.\.\/revm\/crates\/revm"/s/^/# /
    /^revm-bytecode = { path = "\.\.\/revm\/crates\/bytecode"/s/^/# /
    /^revm-context = { path = "\.\.\/revm\/crates\/context"/s/^/# /
    /^revm-context-interface = { path = "\.\.\/revm\/crates\/context\/interface"/s/^/# /
    /^revm-database = { path = "\.\.\/revm\/crates\/database"/s/^/# /
    /^revm-database-interface = { path = "\.\.\/revm\/crates\/database\/interface"/s/^/# /
    /^revm-handler = { path = "\.\.\/revm\/crates\/handler"/s/^/# /
    /^revm-inspector = { path = "\.\.\/revm\/crates\/inspector"/s/^/# /
    /^revm-interpreter = { path = "\.\.\/revm\/crates\/interpreter"/s/^/# /
    /^revm-precompile = { path = "\.\.\/revm\/crates\/precompile"/s/^/# /
    /^revm-primitives = { path = "\.\.\/revm\/crates\/primitives"/s/^/# /
    /^revm-state = { path = "\.\.\/revm\/crates\/state"/s/^/# /
    /^op-revm = { version = "3.0.1", path = "\.\.\/revm\/crates\/optimism"/s/^/# /
    ' Cargo.toml
    
    sed -i '' '
    /^# revm = { version = "22.0.1", git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^# //
    /^# revm-bytecode = { git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^# //
    /^# revm-context = { git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^# //
    /^# revm-context-interface = { git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^# //
    /^# revm-database = { git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^# //
    /^# revm-database-interface = { git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^# //
    /^# revm-handler = { git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^# //
    /^# revm-inspector = { git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^# //
    /^# revm-interpreter = { git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^# //
    /^# revm-precompile = { git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^# //
    /^# revm-primitives = { git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^# //
    /^# revm-state = { git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^# //
    /^# op-revm = { version = "3.0.1", git = "https:\/\/github.com\/0xFacet\/facet-revm"/s/^# //
    ' Cargo.toml
    
    echo "✅ Switched to GitHub REVM (0xFacet/facet-revm)"
fi

echo ""
echo "🔄 Running cargo update to refresh dependencies..."
cargo clean
cargo check -p kona-executor

echo ""
echo "✅ Done! REVM dependencies are now using: $MODE"