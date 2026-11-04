#!/bin/bash
# Startup script of the Spot VM; it runs on every boot, including after a recreate. It prepares the persistent
# data disk, installs the few OS packages, watches for the preemption notice, and starts the job when the
# orchestrator has delivered the source (/mnt/work/START). The job itself resumes from its step markers.
exec > >(tee -a /var/log/nata-startup.log) 2>&1
set -u
MNT=/mnt/work
DEV=/dev/disk/by-id/google-work
for _ in $(seq 1 90); do [ -e "$DEV" ] && break; sleep 2; done
blkid "$DEV" > /dev/null 2>&1 || mkfs.ext4 -q -F "$DEV"
mkdir -p "$MNT"
mountpoint -q "$MNT" || mount -o discard,defaults "$DEV" "$MNT"
id nata > /dev/null 2>&1 || useradd -m -s /bin/bash nata
chown nata:nata "$MNT"
export DEBIAN_FRONTEND=noninteractive
command -v cc > /dev/null || { apt-get update -qq && apt-get install -y -qq build-essential curl git ca-certificates pkg-config; }

# Spot VMs get a notice about 30 seconds before they are reclaimed: tell the job, so it can leave clean markers.
nohup bash -c 'while sleep 3; do
  [ "$(curl -s -H "Metadata-Flavor: Google" http://metadata.google.internal/computeMetadata/v1/instance/preempted)" = TRUE ] \
    && { pkill -TERM -u nata -f job.sh; sync; break; }
done' > /dev/null 2>&1 &

# Wait for the orchestrator to deliver the source, then run the job (once) as the unprivileged user.
for _ in $(seq 1 600); do [ -f "$MNT/START" ] && break; sleep 2; done
# Jobs that need root-only setup get it here, because the job itself runs as the unprivileged user `nata`.
# The `perf` job profiles with perf: install it and allow unprivileged profiling (Debian sets perf_event_paranoid to 3).
if [ "$(cat "$MNT/JOBNAME" 2> /dev/null)" = perf ]; then
  command -v perf > /dev/null || apt-get install -y -qq linux-perf || apt-get install -y -qq "linux-perf-$(uname -r | cut -d. -f1-2)"
  sysctl -w kernel.perf_event_paranoid=1 kernel.kptr_restrict=0 > /dev/null 2>&1 || true
fi
# The Kani jobs record the peak memory of CBMC with GNU time.
case "$(cat "$MNT/JOBNAME" 2> /dev/null)" in kani*) [ -x /usr/bin/time ] || apt-get install -y -qq time ;; esac
if [ -f "$MNT/START" ] && ! pgrep -u nata -f "scripts/gcp/job.sh" > /dev/null; then
  su - nata -c "mkdir -p $MNT/logs && cd $MNT && WORK=$MNT nohup bash src/scripts/gcp/job.sh >> $MNT/logs/job.out 2>&1 &"
fi
