#!/usr/bin/env python3
"""Fix the broken make_result calls by replacing ', 0)' pattern with proper 4th arg."""

with open('core/src/processor.rs', 'r') as f:
    content = f.read()

# The broken pattern is:
#   Some(format!("...{}", e)),
#            , 0);
# where the ", 0)" was placed at the position of the original ")"
# We need it to be:
#   Some(format!("...{}", e)),
#            0,
#            );

# First, let's revert all the broken ", 0)" insertions back to just ")"
# The pattern is: text\n<whitespace>, 0)<semi>
# which should become: text\n<whitespace>0,\n<same_whitespace>)<semi>

import re

# Pattern: something ending with comma+newline+spaces followed by ", 0)"
# Replace ", 0)" at end of make_result calls with proper 4th arg insertion
# Actually, let me just add the arg properly by finding make_result calls that have ", 0)" at the end

# Step 1: Revert the broken changes - remove the ", 0" additions
content = content.replace(', 0)', ')')

# Step 2: Now re-add properly: for each make_result call, add `0` as 4th arg
# Strategy: find make_result( and its matching ), insert "0, " before the )

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
        result_parts.append(content[i:idx + len(needle)])
        i = idx + len(needle)
        continue

    # Find the opening paren
    paren_start = idx + len(needle) - 1  # position of '('
    assert content[paren_start] == '(', f"Expected '(' at {paren_start}"

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
                # j is the position of the closing ')'
                # Check if it's single-line or multi-line
                call_text = content[paren_start:j+1]
                if '\n' in call_text:
                    # Multi-line: the ) is on its own line with indentation
                    # Find the indentation of the closing )
                    close_line_start = content.rfind('\n', paren_start, j) + 1
                    indent = ''
                    k = close_line_start
                    while k < j and content[k] == ' ':
                        indent += ' '
                        k += 1
                    # Insert "    0,\n" before the indent+)
                    result_parts.append(content[i:close_line_start])
                    result_parts.append(indent + '    0,\n')
                    result_parts.append(indent + ')')
                else:
                    # Single-line: insert ", 0" before the closing )
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

with open('core/src/processor.rs', 'w') as f:
    f.write(new_content)

print(f"Fixed {changes} call sites")
