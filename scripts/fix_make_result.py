#!/usr/bin/env python3
"""Add compute_units_used=0 as 4th argument to all self.make_result() calls."""

import sys

filepath = 'core/src/processor.rs'
with open(filepath, 'r') as f:
    content = f.read()

result_parts = []
i = 0
changes = 0
needle = 'self.make_result('

while i < len(content):
    idx = content.find(needle, i)
    if idx == -1:
        result_parts.append(content[i:])
        break

    # Check if this is the function definition line
    line_start = content.rfind('\n', 0, idx) + 1
    line_end = content.find('\n', idx)
    line = content[line_start:line_end]
    if 'fn make_result' in line:
        # Skip the definition
        result_parts.append(content[i:idx + len(needle)])
        i = idx + len(needle)
        continue

    # Find the opening paren
    paren_start = idx + len(needle) - 1  # position of '('
    assert content[paren_start] == '(', f"Expected '(' at pos {paren_start}, got '{content[paren_start]}'"

    # Count parens to find matching close
    depth = 0
    j = paren_start
    found = False
    while j < len(content):
        ch = content[j]
        if ch == '(':
            depth += 1
        elif ch == ')':
            depth -= 1
            if depth == 0:
                # j is the position of the closing paren
                # Insert ", 0" before it
                result_parts.append(content[i:j])
                result_parts.append(', 0)')
                i = j + 1
                changes += 1
                found = True
                break
        j += 1

    if not found:
        result_parts.append(content[i:])
        break

new_content = ''.join(result_parts)

with open(filepath, 'w') as f:
    f.write(new_content)

print(f"Modified {changes} call sites")
