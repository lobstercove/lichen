#!/bin/bash
cd "$(dirname "$0")"
exec ./target/release/moltchain-validator "$@"
