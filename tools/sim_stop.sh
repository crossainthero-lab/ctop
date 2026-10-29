#!/usr/bin/env bash
# stop the simulation stack started by sim_stack.sh (patterns are anchored to the start of the command line)
pkill -f '^python3 -I .*/tools/fake_(controller|camera)\.py' ; pkill -f '^[^ ]*/target/debug/ctop --serve' ; pkill -f '^bash tools/sim_stack\.sh'
exit 0
