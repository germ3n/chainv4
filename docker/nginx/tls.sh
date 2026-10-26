#!/bin/sh
# Runs before nginx starts, from the nginx image's /docker-entrypoint.d. It renders the site's
# config, and puts a certificate where nginx reads it, /etc/chainv4/tls, by TLS_MODE:
#
#   self-signed  one made here for SERVER_NAME on the first start, and kept in the tls volume
#   provided     yours: fullchain.pem and privkey.pem in /etc/chainv4/provided (docker/certs)
#   letsencrypt  certbot's for SERVER_NAME, once it has one, with a self-signed stand-in until
#                then; nginx takes each new one, renewals included, within a minute
set -eu

tls=/etc/chainv4/tls
mode=${TLS_MODE:-self-signed}
name=${SERVER_NAME:-localhost}
mkdir -p "$tls"

# A certificate for this name alone, by IP address or DNS name, as it is. It is no CA, which
# rustls, and so chainv4 send --ca-cert, refuses to take as a server's own certificate.
self_sign() {
    case "$name" in
        *[!0-9.]*) alt="DNS:$name" ;;
        *) alt="IP:$name" ;;
    esac

    openssl req -x509 -newkey rsa:2048 -nodes -days 825 -subj "/CN=$name" \
        -addext "subjectAltName=$alt" -addext "basicConstraints=critical,CA:FALSE" \
        -addext "extendedKeyUsage=serverAuth" -keyout "$tls/privkey.pem" -out "$tls/fullchain.pem" 2> /dev/null
    echo "[tls] made a self-signed certificate for $name"
}

case "$mode" in
    self-signed)
        # Made again when it is missing, about to expire, or made by an older tls.sh as a CA.
        if [ ! -s "$tls/fullchain.pem" ] || ! openssl x509 -in "$tls/fullchain.pem" -noout -checkend 86400 > /dev/null \
            || openssl x509 -in "$tls/fullchain.pem" -noout -text | grep -q 'CA:TRUE'; then
            self_sign
        fi
        ;;
    provided)
        provided=/etc/chainv4/provided

        if [ ! -s "$provided/fullchain.pem" ] || [ ! -s "$provided/privkey.pem" ]; then
            echo "[tls] TLS_MODE=provided needs fullchain.pem and privkey.pem in docker/certs" >&2
            exit 1
        fi

        cp "$provided/fullchain.pem" "$provided/privkey.pem" "$tls/"
        echo "[tls] using the certificate in docker/certs"
        ;;
    letsencrypt)
        live=/etc/letsencrypt/live/$name

        if [ -s "$live/fullchain.pem" ]; then
            cp "$live/fullchain.pem" "$live/privkey.pem" "$tls/"
            echo "[tls] using Let's Encrypt's certificate for $name"
        else
            self_sign
            echo "[tls] standing in until certbot has one from Let's Encrypt"
        fi

        # Each new certificate certbot gets, nginx takes, by itself.
        (
            while sleep 60; do
                if [ -s "$live/fullchain.pem" ] && ! cmp -s "$live/fullchain.pem" "$tls/fullchain.pem"; then
                    cp "$live/fullchain.pem" "$live/privkey.pem" "$tls/"
                    nginx -s reload && echo "[tls] took Let's Encrypt's new certificate for $name"
                fi
            done
        ) &
        ;;
    *)
        echo "[tls] TLS_MODE is self-signed, provided or letsencrypt, not $mode" >&2
        exit 1
        ;;
esac

# Strict-Transport-Security only for certificates browsers trust: a self-signed one would lock
# visitors out once it changed.
case "$mode" in
    self-signed) HSTS= ;;
    *) HSTS='add_header Strict-Transport-Security "max-age=31536000" always;' ;;
esac

case "${HTTPS_PORT:-443}" in
    443) HTTPS_PORT_SUFFIX= ;;
    *) HTTPS_PORT_SUFFIX=":$HTTPS_PORT" ;;
esac

export SERVER_NAME="$name" NODE_UPSTREAM="${NODE_UPSTREAM:-node:8080}" HSTS HTTPS_PORT_SUFFIX
# shellcheck disable=SC2016
envsubst '${SERVER_NAME} ${NODE_UPSTREAM} ${HSTS} ${HTTPS_PORT_SUFFIX}' \
    < /etc/nginx/chainv4.conf.template > /etc/nginx/conf.d/default.conf
