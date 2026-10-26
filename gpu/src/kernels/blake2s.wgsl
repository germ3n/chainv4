// BLAKE2s-256 of the 52-byte message: one block, no key, a 32-byte digest.

const IV: array<u32, 8> = array<u32, 8>(
    0x6A09E667u, 0xBB67AE85u, 0x3C6EF372u, 0xA54FF53Au, 0x510E527Fu, 0x9B05688Cu, 0x1F83D9ABu, 0x5BE0CD19u
);

const SIGMA: array<u32, 160> = array<u32, 160>(
    0u, 1u, 2u, 3u, 4u, 5u, 6u, 7u, 8u, 9u, 10u, 11u, 12u, 13u, 14u, 15u,
    14u, 10u, 4u, 8u, 9u, 15u, 13u, 6u, 1u, 12u, 0u, 2u, 11u, 7u, 5u, 3u,
    11u, 8u, 12u, 0u, 5u, 2u, 15u, 13u, 10u, 14u, 3u, 6u, 7u, 1u, 9u, 4u,
    7u, 9u, 3u, 1u, 13u, 12u, 11u, 14u, 2u, 6u, 5u, 10u, 4u, 0u, 15u, 8u,
    9u, 0u, 5u, 7u, 2u, 4u, 10u, 15u, 14u, 1u, 11u, 12u, 6u, 8u, 3u, 13u,
    2u, 12u, 6u, 10u, 0u, 11u, 8u, 3u, 4u, 13u, 7u, 5u, 15u, 14u, 1u, 9u,
    12u, 5u, 1u, 15u, 14u, 13u, 4u, 10u, 0u, 7u, 6u, 3u, 9u, 2u, 8u, 11u,
    13u, 11u, 7u, 14u, 12u, 1u, 3u, 9u, 5u, 0u, 15u, 4u, 8u, 6u, 2u, 10u,
    6u, 15u, 14u, 9u, 11u, 3u, 0u, 8u, 12u, 2u, 13u, 7u, 1u, 4u, 10u, 5u,
    10u, 2u, 8u, 4u, 7u, 6u, 1u, 5u, 15u, 11u, 9u, 14u, 3u, 12u, 13u, 0u
);

fn g(v: ptr<function, array<u32, 16>>, a: u32, b: u32, c: u32, d: u32, x: u32, y: u32) {
    (*v)[a] = (*v)[a] + (*v)[b] + x;
    (*v)[d] = rotr((*v)[d] ^ (*v)[a], 16u);
    (*v)[c] = (*v)[c] + (*v)[d];
    (*v)[b] = rotr((*v)[b] ^ (*v)[c], 12u);
    (*v)[a] = (*v)[a] + (*v)[b] + y;
    (*v)[d] = rotr((*v)[d] ^ (*v)[a], 8u);
    (*v)[c] = (*v)[c] + (*v)[d];
    (*v)[b] = rotr((*v)[b] ^ (*v)[c], 7u);
}

fn digest(m: ptr<function, array<u32, 16>>, out: ptr<function, array<u32, 8>>) {
    var h: array<u32, 8>;
    var v: array<u32, 16>;

    for (var i = 0u; i < 8u; i++) {
        h[i] = IV[i];
    }

    // The parameter block: a 32-byte digest, fanout 1, depth 1.
    h[0] = h[0] ^ 0x01010020u;

    for (var i = 0u; i < 8u; i++) {
        v[i] = h[i];
        v[i + 8u] = IV[i];
    }

    // 52 bytes in, and the last block.
    v[12] = v[12] ^ 52u;
    v[14] = ~v[14];

    for (var r = 0u; r < 10u; r++) {
        let s = r * 16u;
        g(&v, 0u, 4u, 8u, 12u, (*m)[SIGMA[s]], (*m)[SIGMA[s + 1u]]);
        g(&v, 1u, 5u, 9u, 13u, (*m)[SIGMA[s + 2u]], (*m)[SIGMA[s + 3u]]);
        g(&v, 2u, 6u, 10u, 14u, (*m)[SIGMA[s + 4u]], (*m)[SIGMA[s + 5u]]);
        g(&v, 3u, 7u, 11u, 15u, (*m)[SIGMA[s + 6u]], (*m)[SIGMA[s + 7u]]);
        g(&v, 0u, 5u, 10u, 15u, (*m)[SIGMA[s + 8u]], (*m)[SIGMA[s + 9u]]);
        g(&v, 1u, 6u, 11u, 12u, (*m)[SIGMA[s + 10u]], (*m)[SIGMA[s + 11u]]);
        g(&v, 2u, 7u, 8u, 13u, (*m)[SIGMA[s + 12u]], (*m)[SIGMA[s + 13u]]);
        g(&v, 3u, 4u, 9u, 14u, (*m)[SIGMA[s + 14u]], (*m)[SIGMA[s + 15u]]);
    }

    for (var i = 0u; i < 8u; i++) {
        (*out)[i] = bswap(h[i] ^ v[i] ^ v[i + 8u]);
    }
}
