#!/bin/sh
# Gets a certificate from Let's Encrypt for SERVER_NAME, answering its HTTP-01 challenge through
# nginx's webroot, then renews it twice a day. nginx takes each new one within a minute.
# LETSENCRYPT_STAGING=1 asks Let's Encrypt's staging service, for trying it out.
set -eu
: "${SERVER_NAME:?set SERVER_NAME to the domain to get a certificate for}"
: "${LETSENCRYPT_EMAIL:?set LETSENCRYPT_EMAIL to an address for expiry notices}"

staging=

if [ "${LETSENCRYPT_STAGING:-0}" = 1 ]; then
    staging=--staging
fi

until [ -s "/etc/letsencrypt/live/$SERVER_NAME/fullchain.pem" ]; do
    # shellcheck disable=SC2086
    certbot certonly --webroot -w /var/www/certbot -d "$SERVER_NAME" --cert-name "$SERVER_NAME" \
        --email "$LETSENCRYPT_EMAIL" --agree-tos --no-eff-email --non-interactive $staging \
        || { echo "[certbot] no certificate yet; trying again in 5 minutes"; sleep 300; }
done

while :; do
    sleep 43200
    certbot renew --webroot -w /var/www/certbot --quiet || echo "[certbot] renewing failed; trying again in 12 hours"
done
