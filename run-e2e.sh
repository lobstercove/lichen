#!/bin/bash
cd /Users/johnrobin/.openclaw/workspace/moltchain
python3 tests/comprehensive-e2e.py > /tmp/e2e-final-run.txt 2>&1
echo "E2E_EXIT=$?" >> /tmp/e2e-final-run.txt
tail -25 /tmp/e2e-final-run.txt
