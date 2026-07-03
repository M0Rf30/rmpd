#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"

# Enable cross-compile support if configured.
_CC_ENV="$(dirname "$0")/scripts/cross-compile-env.sh"
if [ -f "$_CC_ENV" ]; then
    # shellcheck disable=SC1090
    . "$_CC_ENV"
else
    echo "Not using cross-compilation (${_CC_ENV} does not exist)"
fi

if [ -n "${DIST:-}" ]; then
    echo "Using distribution from DIST environment variable: $DIST"
    DIST_ARG="--dist=$DIST"
else
    echo "No DIST environment variable set, using sbuild default"
    DIST_ARG=""
fi

if [ -f target ]; then
    echo "Removing previous build target"
    rm -f target
fi

if command -v sbuild >/dev/null 2>&1; then
    sbuild --chroot-mode=unshare \
           --enable-network \
           --no-clean-source \
           $DIST_ARG
else
    echo "sbuild not found; falling back to local dpkg-buildpackage"
    dpkg-buildpackage -b -us -uc
fi