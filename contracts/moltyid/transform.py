#!/usr/bin/env python3
"""
Transform all from_raw_parts in moltyid/src/lib.rs to owned-copy pattern.
CORRECT approach: Phase 1 converts, then re-indexes before Phase 2.
"""
import re
from pathlib import Path

filepath = Path(__file__).resolve().parent / 'src' / 'lib.rs'

with open(filepath, 'r') as f:
    orig_lines = f.read().split('\n')

# ── Phase 1: Convert from_raw_parts ─────────────────────────────────────────
pat32 = re.compile(
    r'^(\s+)let\s+(?:mut\s+)?(\w+)\s*=\s*unsafe\s*\{\s*core::slice::from_raw_parts\((\w+),\s*32\)\s*\};')
patV = re.compile(
    r'^(\s+)let\s+(?:mut\s+)?(\w+)\s*=\s*unsafe\s*\{\s*core::slice::from_raw_parts\((\w+),\s*(.+?)\)\s*\};')

# Track conversions by variable name (we'll re-index after)
# Store: (original_line_idx, var_name, type)
orig_conversions = []

for i, line in enumerate(orig_lines):
    m = pat32.match(line)
    if m:
        indent, var, ptr = m.group(1), m.group(2), m.group(3)
        orig_conversions.append((i, var, 'fixed32', indent, ptr, '32'))
        continue
    m = patV.match(line)
    if m:
        indent, var, ptr, le = m.group(1), m.group(2), m.group(3), m.group(4).strip()
        orig_conversions.append((i, var, 'varlen', indent, ptr, le))

print(f"Phase 1: {len(orig_conversions)} conversions")

# Build conversion map with replacement text
conv_map = {}
for (li, var, vty, indent, ptr, le) in orig_conversions:
    if vty == 'fixed32':
        conv_map[li] = [
            f"{indent}let mut {var} = [0u8; 32];",
            f"{indent}unsafe {{ core::ptr::copy_nonoverlapping({ptr}, {var}.as_mut_ptr(), 32); }}"
        ]
    else:
        conv_map[li] = [
            f"{indent}let mut {var} = alloc::vec![0u8; {le}];",
            f"{indent}unsafe {{ core::ptr::copy_nonoverlapping({ptr}, {var}.as_mut_ptr(), {le}); }}"
        ]

# Apply phase 1: each converted line becomes 2 lines
new_lines = []
for i, line in enumerate(orig_lines):
    if i in conv_map:
        new_lines.extend(conv_map[i])
    else:
        new_lines.append(line)

# ── Re-index everything on the new lines ─────────────────────────────────────
func_starts = []
for i, line in enumerate(new_lines):
    s = line.strip()
    if s.startswith('pub extern "C" fn ') or (s.startswith('fn ') and not s.startswith('fn(')):
        func_starts.append(i)

def get_func_for_line(idx):
    best = -1
    for fs in func_starts:
        if fs <= idx: best = fs
    return best

# Find all conversion variables in the NEW lines
# Look for the pattern we just created: "let mut VAR = [0u8; 32];" or "let mut VAR = alloc::vec![0u8; ...];"
conversion_new = {}  # (func_start, var_name) -> 'fixed32'|'varlen'
for i, line in enumerate(new_lines):
    m = re.match(r'^\s+let mut (\w+) = \[0u8; 32\];$', line)
    if m:
        var = m.group(1)
        # Next line should be unsafe copy_nonoverlapping
        if i+1 < len(new_lines) and 'copy_nonoverlapping' in new_lines[i+1]:
            fs = get_func_for_line(i)
            conversion_new[(fs, var)] = 'fixed32'
            continue
    m = re.match(r'^\s+let mut (\w+) = alloc::vec!\[0u8; .+\];$', line)
    if m:
        var = m.group(1)
        if i+1 < len(new_lines) and 'copy_nonoverlapping' in new_lines[i+1]:
            fs = get_func_for_line(i)
            conversion_new[(fs, var)] = 'varlen'

print(f"Re-indexed: {len(conversion_new)} converted variables")

# Build per-function variable maps
func_vars = {}
for (fs, vn), vt in conversion_new.items():
    func_vars.setdefault(fs, {})[vn] = vt

# ── Phase 2: Downstream fixes ───────────────────────────────────────────────

def is_comment_line(line):
    s = line.strip()
    return s.startswith('//') or s.startswith('///') or s.startswith('*') or s.startswith('/*')

def is_conversion_line(line):
    return 'copy_nonoverlapping' in line or \
           ('= [0u8; 32]' in line and 'let mut' in line) or \
           'alloc::vec![0u8;' in line

def add_ref_to_fn_arg(line, fn_name, var_name):
    """In a line containing fn_name(...var_name...), add & before var_name."""
    if fn_name not in line or var_name not in line:
        return line
    
    result = ''
    idx = 0
    search_str = fn_name + '('
    
    while idx < len(line):
        # Try to find fn_name( starting from idx
        pos = line.find(search_str, idx)
        if pos == -1:
            # Also try .fn_name( (method call)
            pos2 = line.find('.' + search_str, idx)
            if pos2 == -1:
                result += line[idx:]
                break
            pos = pos2 + 1  # skip the dot
        
        result += line[idx:pos]
        
        call_start = pos + len(fn_name)
        if call_start >= len(line) or line[call_start] != '(':
            result += fn_name
            idx = pos + len(fn_name)
            continue
        
        # Find matching close paren
        depth = 1
        j = call_start + 1
        while j < len(line) and depth > 0:
            if line[j] == '(': depth += 1
            elif line[j] == ')': depth -= 1
            j += 1
        
        args_str = line[call_start+1:j-1]
        
        # Add & before bare var_name (not preceded by &, not followed by word char or [)
        pattern = r'(?<!&)\b' + re.escape(var_name) + r'\b(?![\w\[])'
        fixed_args = re.sub(pattern, '&' + var_name, args_str)
        
        result += fn_name + '(' + fixed_args + ')'
        idx = j
    
    return result

# All functions that take &[u8]
SLICE_FNS = [
    'is_mid_admin', 'identity_key', 'reputation_key', 'hex_encode_addr',
    'skill_key', 'vouch_key', 'name_key', 'name_reverse_key',
    'name_auction_key', 'endpoint_key', 'metadata_key', 'availability_key',
    'rate_key', 'delegation_key', 'recovery_guardians_key',
    'recovery_nonce_key', 'recovery_candidate_key', 'recovery_approval_key',
    'recovery_nonce', 'has_vouched_for', 'is_configured_guardian',
    'apply_decay_to_identity_record', 'check_achievements',
    'vouch_cooldown_key', 'register_cooldown_key', 'validate_molt_name',
    'is_reserved_name', 'is_premium_name', 'is_zero_address',
    'has_active_permission', 'recovery_approval_count',
    'name_registration_cost',
]

# storage_set needs & for second arg, but first arg is key which may be &key already
# We'll use add_ref_to_fn_arg for storage_set which handles all args
SLICE_FNS.append('storage_set')

# SDK/utility functions
SLICE_FNS.extend(['set_return_data', 'from_utf8'])

# Methods
SLICE_METHODS = ['extend_from_slice', 'copy_from_slice']

result_lines = []
cur_func = -1
cur_vars = {}

for i, line in enumerate(new_lines):
    func = get_func_for_line(i)
    if func != cur_func:
        cur_func = func
        cur_vars = func_vars.get(func, {})
    
    if not cur_vars or is_comment_line(line) or is_conversion_line(line):
        result_lines.append(line)
        continue
    
    modified = line
    
    for var_name, vtype in cur_vars.items():
        if var_name not in modified:
            continue
        
        # Skip let declarations
        if re.search(r'let\s+(mut\s+)?' + re.escape(var_name) + r'\s*=', modified):
            continue
        
        is_f32 = (vtype == 'fixed32')
        
        # Fix function arguments
        for fn_name in SLICE_FNS:
            modified = add_ref_to_fn_arg(modified, fn_name, var_name)
        
        # Fix method arguments (.extend_from_slice, .copy_from_slice)
        for meth in SLICE_METHODS:
            # Pattern: .method(var_name) -> .method(&var_name)
            pat = re.compile(r'(\.' + re.escape(meth) + r'\()(' + re.escape(var_name) + r')(\))')
            modified = pat.sub(r'\1&\2\3', modified)
        
        # Fix moltchain_sdk::set_return_data
        if 'moltchain_sdk::set_return_data' in modified:
            modified = add_ref_to_fn_arg(modified, 'moltchain_sdk::set_return_data', var_name)
        
        # Fix comparisons for fixed32
        if is_f32:
            # VAR != admin.as_slice() -> VAR[..] != admin[..]
            modified = re.sub(
                r'\b' + re.escape(var_name) + r'\b\s*!=\s*(\w+)\.as_slice\(\)',
                var_name + r'[..] != \1[..]', modified)
            modified = re.sub(
                r'\b' + re.escape(var_name) + r'\b\s*==\s*(\w+)\.as_slice\(\)',
                var_name + r'[..] == \1[..]', modified)
            
            # Cross-variable: VAR == OTHER_F32 -> VAR[..] == OTHER_F32[..]
            for ov, ot in cur_vars.items():
                if ov == var_name or ot != 'fixed32': continue
                modified = re.sub(
                    r'\b' + re.escape(var_name) + r'\b\s*==\s*\b' + re.escape(ov) + r'\b(?!\[)',
                    var_name + '[..] == ' + ov + '[..]', modified)
                modified = re.sub(
                    r'\b' + re.escape(var_name) + r'\b\s*!=\s*\b' + re.escape(ov) + r'\b(?!\[)',
                    var_name + '[..] != ' + ov + '[..]', modified)
            
            # &record[0..32] != VAR -> record[0..32] != VAR[..]
            modified = re.sub(
                r'(&?record\[0\.\.32\])\s*!=\s*\b' + re.escape(var_name) + r'\b(?!\[)',
                r'\1 != ' + var_name + '[..]', modified)
            modified = re.sub(
                r'(&?record\[0\.\.32\])\s*==\s*\b' + re.escape(var_name) + r'\b(?!\[)',
                r'\1 == ' + var_name + '[..]', modified)
            
            # Address(VAR.try_into().unwrap()) -> Address(VAR)
            modified = re.sub(
                r'Address\(' + re.escape(var_name) + r'\.try_into\(\)\.unwrap\(\)\)',
                'Address(' + var_name + ')', modified)
    
    result_lines.append(modified)

content = '\n'.join(result_lines)

# ── Phase 3: Special fixes ──────────────────────────────────────────────────
# Guardian array: change type from [&[u8]; N] to [[u8; 32]; N]
content = content.replace(
    'let guardians: [&[u8]; RECOVERY_GUARDIAN_COUNT]',
    'let guardians: [[u8; 32]; RECOVERY_GUARDIAN_COUNT]')

# Fix for guardian in guardians { ... } iteration
# With [[u8; 32]; 5], need .iter()
content = content.replace(
    'for guardian in guardians {',
    'for guardian in guardians.iter() {')

# Remove double-refs: &&var -> &var (only for converted var names)
all_var_names = set(vn for (_, vn) in conversion_new.keys())
for vn in all_var_names:
    content = re.sub(r'&&' + re.escape(vn) + r'\b', '&' + vn, content)

with open(filepath, 'w') as f:
    f.write(content)

print(f"\nRemaining from_raw_parts: {content.count('from_raw_parts')}")
print(f"copy_nonoverlapping count: {content.count('copy_nonoverlapping')}")
print("Done!")
