#!/bin/sh

INTERVAL=${INTERVAL:-300}

while true; do
    date +"%Y-%m-%d %H:%M:%S"
    python3 heartbeat.py
    printf "\nSleeping for %s\n\n" ${INTERVAL}
    sleep ${INTERVAL}
done
