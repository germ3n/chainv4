# nginx in front of a chainv4 node: HTTPS for its API, its explorer and its wallet, with a
# certificate you give it, one from Let's Encrypt, or one it makes itself (see tls.sh).
FROM docker.io/library/nginx:1.28-alpine
RUN apk add --no-cache openssl
COPY chainv4.conf.template /etc/nginx/chainv4.conf.template
COPY --chmod=0755 tls.sh /docker-entrypoint.d/40-chainv4-tls.sh
