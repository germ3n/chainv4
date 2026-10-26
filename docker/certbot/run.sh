#!/bin/sh
# Gets a certificate from Let's Encrypt for SERVER_NAME, answering its HTTP-01 challenge through
# nginx's webroot, then renews it twice a day. nginx takes each new one within a minute.
# LETSENCRYPT_STAGING=1 asks Let's Encrypt's staging service, for trying it out. LETSENCRYPT_EMAIL
# is optional: Let's Encrypt sends no expiry notices since 2025, so the account needs no address.
set -eu
: "${SERVER_NAME:?set SERVER_NAME to the domain to get a certificate for}"

staging=

if [ "${LETSENCRYPT_STAGING:-0}" = 1 ]; then
    staging=--staging
fi

account=--register-unsafely-without-email

if [ -n "${LETSENCRYPT_EMAIL:-}" ]; then
    account="--email $LETSENCRYPT_EMAIL"
fi

until [ -s "/etc/letsencrypt/live/$SERVER_NAME/fullchain.pem" ]; do
    # shellcheck disable=SC2086
    certbot certonly --webroot -w /var/www/certbot -d "$SERVER_NAME" --cert-name "$SERVER_NAME" \
        $account --agree-tos --no-eff-email --non-interactive $staging \
        || { echo "[certbot] no certificate yet; trying again in 5 minutes"; sleep 300; }
done

while :; do
    sleep 43200
    certbot renew --webroot -w /var/www/certbot --quiet || echo "[certbot] renewing failed; trying again in 12 hours"
done
