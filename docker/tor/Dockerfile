# Tor in front of a chainv4 node: an onion service for its API, its explorer and its wallet, and
# with TOR_MINING=1 its mining port. Only with --profile tor; docker/pow.sh up turns it on when
# .env says TOR=1. The onion's key is in the tor volume, so its address outlives every rebuild.
FROM docker.io/library/alpine:3.22
RUN apk add --no-cache tor && mkdir -p /onion && chown tor /onion
COPY --chmod=0755 entrypoint.sh /entrypoint.sh
USER tor
ENTRYPOINT ["/entrypoint.sh"]
