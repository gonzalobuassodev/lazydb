#!/bin/sh
set -eu

mongo_wait_deadline=$(( $(date +%s) + 90 ))
while [ "$(date +%s)" -lt "$mongo_wait_deadline" ]; do
  if mongosh "${LAZYDB_TEST_MONGODB_URL:?}" --quiet --eval 'quit(db.hello().isWritablePrimary ? 0 : 1)' >/dev/null 2>&1; then
    exit 0
  fi
  sleep 2
done
echo "MongoDB replica set did not become primary before timeout" >&2
exit 1
