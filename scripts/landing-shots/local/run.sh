#!/usr/bin/env bash
# Regenerate the local app's tour screenshots in a throwaway Kubernetes pod,
# never on this machine: the pod builds the server and the web app from this
# checkout, serves a made-up home and photographs it (shoot.sh). The PNGs are
# copied back to <out-dir> and the pod is deleted.
#
#   scripts/landing-shots/local/run.sh [<out-dir>]      (default: ./landing-shots-local)
#
# kubectl's current context and namespace are used unless KUBE_CONTEXT /
# KUBE_NAMESPACE say otherwise. POD names the pod (default
# droppen-landing-shots-local), POD_PRIORITY_CLASS gives it a priority class,
# KEEP_POD=1 leaves it running afterwards (for `shoot.sh lang <lang>` re-runs),
# LANGS picks the languages (default `en zh-CN`).
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
OUT=${1:-./landing-shots-local}
POD=${POD:-droppen-landing-shots-local}
IMAGE=rust:1.98.1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e
K=(kubectl ${KUBE_CONTEXT:+--context "$KUBE_CONTEXT"} ${KUBE_NAMESPACE:+-n "$KUBE_NAMESPACE"})

cleanup() { [ "${KEEP_POD:-}" = 1 ] || "${K[@]}" delete pod "$POD" --ignore-not-found --wait=false >/dev/null; }
trap cleanup EXIT

if ! "${K[@]}" get pod "$POD" >/dev/null 2>&1; then
  # `hostname: macbook`: the app shows the host it runs on.
  cat <<YAML | "${K[@]}" create -f - >/dev/null
apiVersion: v1
kind: Pod
metadata:
  name: $POD
  labels: {app: droppen-landing-shots}
spec:
  hostname: macbook
  restartPolicy: Never
  ${POD_PRIORITY_CLASS:+priorityClassName: $POD_PRIORITY_CLASS}
  automountServiceAccountToken: false
  enableServiceLinks: false
  terminationGracePeriodSeconds: 5
  activeDeadlineSeconds: 14400
  volumes:
    - {name: work, emptyDir: {sizeLimit: 40Gi}}
  containers:
    - name: shots
      image: $IMAGE
      command: [sleep, "14400"]
      resources:
        requests: {cpu: "4", memory: 8Gi, ephemeral-storage: 10Gi}
        limits: {cpu: "16", memory: 32Gi, ephemeral-storage: 40Gi}
      volumeMounts:
        - {name: work, mountPath: /work}
YAML
fi
"${K[@]}" wait --for=condition=Ready "pod/$POD" --timeout=20m >/dev/null

echo "run.sh: uploading $REPO to $POD" >&2
"${K[@]}" exec "$POD" -- bash -c 'rm -rf /work/src /work/out && mkdir -p /work/src'
# macOS's tar would add its extended attributes, which GNU tar warns about per file.
mac=(); tar --version 2>/dev/null | grep -q bsdtar && mac=(--no-xattrs --no-mac-metadata)
COPYFILE_DISABLE=1 tar ${mac[@]+"${mac[@]}"} -C "$REPO" -cf - --exclude ./.git --exclude node_modules --exclude ./server-rs/target \
  --exclude ./desktop/target --exclude ./target --exclude ./web/dist . |
  "${K[@]}" exec -i "$POD" -- tar -x -C /work/src

"${K[@]}" exec "$POD" -- env SRC=/work/src OUT=/work/out LANGS="${LANGS:-en zh-CN}" bash /work/src/scripts/landing-shots/local/shoot.sh all

mkdir -p "$OUT"
"${K[@]}" exec "$POD" -- tar -C /work/out -cf - . | tar -x -C "$OUT"
echo "run.sh: $(ls "$OUT" | wc -l | tr -d ' ') files in $OUT" >&2
