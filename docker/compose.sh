#!/bin/sh
# Runs compose for docker/compose.yaml with Docker or Podman, whichever CONTAINER_ENGINE names:
# docker or podman, from the environment or docker/.env, else the one installed, Docker first.
# Arguments go on to compose as they are:
#
#   docker/compose.sh up -d --build
#   CONTAINER_ENGINE=podman docker/compose.sh --profile letsencrypt up -d --build
#   docker/compose.sh logs -f node
#
# Podman's compose is `podman compose`, or podman-compose where that is missing. Its builds are
# made in Docker's image format, so the node's HEALTHCHECK stays in the image as Docker keeps it.
set -eu

here=$(cd "$(dirname "$0")" && pwd)

say() { printf '[compose] %s\n' "$*" >&2; }
fail() { printf '[compose] %s\n' "$*" >&2; exit 1; }

# The engine: as asked, in the environment or .env, else whichever is here.
engine=${CONTAINER_ENGINE:-}

if [ -z "$engine" ] && [ -s "$here/.env" ]; then
    engine=$(sed -n 's/^CONTAINER_ENGINE=\([a-z]*\).*/\1/p' "$here/.env" | tail -n 1)
fi

if [ -z "$engine" ]; then
    if command -v docker > /dev/null; then engine=docker; elif command -v podman > /dev/null; then engine=podman; fi
fi

case "$engine" in
    docker)
        command -v docker > /dev/null || fail "CONTAINER_ENGINE is docker, but there is no docker here"
        docker compose version > /dev/null 2>&1 || fail "no docker compose: install Docker's compose plugin"
        set -- docker compose "$@"
        ;;
    podman)
        command -v podman > /dev/null || fail "CONTAINER_ENGINE is podman, but there is no podman here"

        if podman compose version > /dev/null 2>&1; then
            set -- podman compose "$@"
        elif command -v podman-compose > /dev/null; then
            set -- podman-compose "$@"
        else
            fail "no compose for podman: install podman-compose, or docker-compose for podman compose to use"
        fi

        export BUILDAH_FORMAT=docker

        # Rootless, it binds no port below the system's unprivileged start, 1024 by default.
        if [ "$(podman info --format '{{.Host.Security.Rootless}}' 2> /dev/null)" = true ]; then
            start=$(cat /proc/sys/net/ipv4/ip_unprivileged_port_start 2> /dev/null || echo 1024)
            low=$(sed -n 's/^HTTP_PORT=\([0-9]*\).*/\1/p' "$here/.env" 2> /dev/null | tail -n 1)
            low=${HTTP_PORT:-${low:-80}}

            if [ "$low" -lt "$start" ]; then
                say "rootless podman cannot bind port $low: run  sudo sysctl net.ipv4.ip_unprivileged_port_start=80"
                say "(and add it to /etc/sysctl.d to keep it), or set HTTP_PORT=8080 and HTTPS_PORT=8443 in .env"
            fi
        fi
        ;;
    "") fail "neither docker nor podman is here: install one, or set CONTAINER_ENGINE" ;;
    *) fail "CONTAINER_ENGINE is docker or podman, not $engine" ;;
esac

cd "$here"
exec "$@"
