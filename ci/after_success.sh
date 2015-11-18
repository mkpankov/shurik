#!/bin/sh

set -e
set -u
set -x

. ./environment

scp ./ci/run.sh user@host:
scp ./target/debug/shurik user@host:

ssh-keyscan gitlab.host >> ~/.ssh/known_hosts
