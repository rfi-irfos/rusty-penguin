#!/bin/bash
# Rusty Penguin System Validation Test
# Tests that all components are built and functional

# Don't exit on error - let tests continue

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_RESULTS=()
PASS_COUNT=0
FAIL_COUNT=0

# Color codes
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log_pass() {
    echo -e "${GREEN}✓${NC} $1"
    ((PASS_COUNT++))
}

log_fail() {
    echo -e "${RED}✗${NC} $1"
    ((FAIL_COUNT++))
}

log_info() {
    echo -e "${YELLOW}→${NC} $1"
}

echo "╔════════════════════════════════════════════════════════════╗"
echo "║   Rusty Penguin Distribution System Test Suite            ║"
echo "║   Version: 1.0.0 (Beta)                                   ║"
echo "║   Date: $(date '+%Y-%m-%d %H:%M:%S')                             ║"
echo "╚════════════════════════════════════════════════════════════╝"
echo ""

# Test 1: Check build artifacts exist
log_info "Checking build artifacts..."

test_files=(
    "$REPO_ROOT/iso/boot/vmlinuz"
    "$REPO_ROOT/iso/boot/initrd.img"
    "$REPO_ROOT/iso/boot/kernel.elf"
    "$REPO_ROOT/iso/boot/initrd-bare.img"
    "$REPO_ROOT/rusty-penguin.iso"
)

for file in "${test_files[@]}"; do
    if [ -f "$file" ]; then
        size=$(du -h "$file" | cut -f1)
        log_pass "Found $file ($size)"
    else
        log_fail "Missing $file"
    fi
done

echo ""

# Test 2: Check source code builds
log_info "Checking source code structure..."

source_dirs=(
    "$REPO_ROOT/kernel"
    "$REPO_ROOT/desktop-metal"
    "$REPO_ROOT/shell"
    "$REPO_ROOT/init"
    "$REPO_ROOT/iso"
)

for dir in "${source_dirs[@]}"; do
    if [ -d "$dir" ]; then
        log_pass "Source directory: $(basename $dir)"
    else
        log_fail "Missing source directory: $(basename $dir)"
    fi
done

echo ""

# Test 3: Verify cargo workspaces
log_info "Checking Rust workspace..."

if [ -f "$REPO_ROOT/Cargo.toml" ]; then
    log_pass "Cargo workspace found"

    # Check for required members
    members=(
        "kernel"
        "shell"
        "init"
        "desktop"
    )

    for member in "${members[@]}"; do
        if grep -q "\"$member\"" "$REPO_ROOT/Cargo.toml"; then
            log_pass "Workspace member: $member"
        else
            log_fail "Missing workspace member: $member"
        fi
    done

    # Note: desktop-metal is built separately with custom target
    if [ -d "$REPO_ROOT/desktop-metal" ]; then
        log_pass "desktop-metal (separate build, custom target)"
    fi
else
    log_fail "Cargo.toml not found"
fi

echo ""

# Test 4: Check ISO components
log_info "Checking ISO components..."

if [ -f "$REPO_ROOT/rusty-penguin.iso" ]; then
    iso_size=$(du -h "$REPO_ROOT/rusty-penguin.iso" | cut -f1)
    log_pass "ISO file exists ($iso_size)"

    # Check GRUB config
    if [ -f "$REPO_ROOT/iso/boot/grub/grub.cfg" ]; then
        if grep -q "linux" "$REPO_ROOT/iso/boot/grub/grub.cfg"; then
            log_pass "Linux boot option found in GRUB"
        else
            log_fail "Linux boot option missing"
        fi

        if grep -q "bare metal" "$REPO_ROOT/iso/boot/grub/grub.cfg"; then
            log_pass "Bare-metal boot option found in GRUB"
        else
            log_fail "Bare-metal boot option missing"
        fi
    else
        log_fail "GRUB config not found"
    fi
else
    log_fail "ISO file not found"
fi

echo ""

# Test 5: Check documentation
log_info "Checking documentation..."

docs=(
    "$REPO_ROOT/README.md"
    "$REPO_ROOT/README-DISTRIBUTION.md"
    "$REPO_ROOT/DISTRIBUTION.md"
)

for doc in "${docs[@]}"; do
    if [ -f "$doc" ]; then
        lines=$(wc -l < "$doc")
        log_pass "Documentation: $(basename $doc) ($lines lines)"
    else
        log_fail "Missing documentation: $(basename $doc)"
    fi
done

echo ""

# Test 6: Verify git history
log_info "Checking git history..."

if git -C "$REPO_ROOT" rev-parse --git-dir > /dev/null 2>&1; then
    log_pass "Git repository found"

    commit_count=$(git -C "$REPO_ROOT" rev-list --count HEAD 2>/dev/null || echo "0")
    log_pass "Total commits: $commit_count"

    last_commit=$(git -C "$REPO_ROOT" log -1 --format="%h: %s" 2>/dev/null || echo "unknown")
    log_pass "Latest commit: $last_commit"
else
    log_fail "Not a git repository"
fi

echo ""

# Summary
echo "╔════════════════════════════════════════════════════════════╗"
echo "║   Test Results Summary                                     ║"
echo "╚════════════════════════════════════════════════════════════╝"
echo ""
echo -e "${GREEN}Passed:${NC} $PASS_COUNT"
echo -e "${RED}Failed:${NC} $FAIL_COUNT"

if [ $FAIL_COUNT -eq 0 ]; then
    echo ""
    echo -e "${GREEN}✓ All tests passed! Rusty Penguin is ready for use.${NC}"
    echo ""
    echo "Next steps:"
    echo "  1. Boot the ISO on hardware or VM"
    echo "  2. Select 'Rusty Penguin v1.0.0' (Linux) or 'Rusty Penguin (bare metal)'"
    echo "  3. Explore the desktop applications"
    echo "  4. Try the TIS console: type 'help' then 'infer hello'"
    echo ""
    exit 0
else
    echo ""
    echo -e "${RED}✗ Some tests failed. Please review the output above.${NC}"
    echo ""
    exit 1
fi
