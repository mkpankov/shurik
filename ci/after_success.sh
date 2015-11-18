#!/bin/sh

set -e
set -u
set -x

. ./environment

HOST='user@host'

ssh "$HOST" 'rm shurik_new'

scp ./ci/run.sh "$HOST:"
scp ./target/debug/shurik "$HOST:shurik_new"
ssh "$HOST" 'ssh-keyscan gitlab.host > ~/.ssh/known_hosts'

ssh "$HOST" 'sh -c "nohup \"./wait_deploy_run.sh\" > shurik.out 2>&1 < /dev/null &"'
