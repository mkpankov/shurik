#!/bin/sh

set -e
set -u
set -x

. ./environment

cargo test
