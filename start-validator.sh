#!/bin/bash
cd /Users/johnrobin/.openclaw/workspace/moltchain
exec ./target/release/moltchain-validator "$@"
