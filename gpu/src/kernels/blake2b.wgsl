// BLAKE2b-256 of the 52-byte message: one block, no key, a 32-byte digest.

const IV: array<vec2<u32>, 8> = array<vec2<u32>, 8>(
    vec2<u32>(0xf3bcc908u, 0x6a09e667u), vec2<u32>(0x84caa73bu, 0xbb67ae85u),
    vec2<u32>(0xfe94f82bu, 0x3c6ef372u), vec2<u32>(0x5f1d36f1u, 0xa54ff53au),
    vec2<u32>(0xade682d1u, 0x510e527fu), vec2<u32>(0x2b3e6c1fu, 0x9b05688cu),
    vec2<u32>(0xfb41bd6bu, 0x1f83d9abu), vec2<u32>(0x137e2179u, 0x5be0cd19u)
);

const SIGMA: array<u32, 192> = array<u32, 192>(
    0u, 1u, 2u, 3u, 4u, 5u, 6u, 7u, 8u, 9u, 10u, 11u, 12u, 13u, 14u, 15u,
    14u, 10u, 4u, 8u, 9u, 15u, 13u, 6u, 1u, 12u, 0u, 2u, 11u, 7u, 5u, 3u,
    11u, 8u, 12u, 0u, 5u, 2u, 15u, 13u, 10u, 14u, 3u, 6u, 7u, 1u, 9u, 4u,
    7u, 9u, 3u, 1u, 13u, 12u, 11u, 14u, 2u, 6u, 5u, 10u, 4u, 0u, 15u, 8u,
    9u, 0u, 5u, 7u, 2u, 4u, 10u, 15u, 14u, 1u, 11u, 12u, 6u, 8u, 3u, 13u,
    2u, 12u, 6u, 10u, 0u, 11u, 8u, 3u, 4u, 13u, 7u, 5u, 15u, 14u, 1u, 9u,
    12u, 5u, 1u, 15u, 14u, 13u, 4u, 10u, 0u, 7u, 6u, 3u, 9u, 2u, 8u, 11u,
    13u, 11u, 7u, 14u, 12u, 1u, 3u, 9u, 5u, 0u, 15u, 4u, 8u, 6u, 2u, 10u,
    6u, 15u, 14u, 9u, 11u, 3u, 0u, 8u, 12u, 2u, 13u, 7u, 1u, 4u, 10u, 5u,
    10u, 2u, 8u, 4u, 7u, 6u, 1u, 5u, 15u, 11u, 9u, 14u, 3u, 12u, 13u, 0u,
    0u, 1u, 2u, 3u, 4u, 5u, 6u, 7u, 8u, 9u, 10u, 11u, 12u, 13u, 14u, 15u,
    14u, 10u, 4u, 8u, 9u, 15u, 13u, 6u, 1u, 12u, 0u, 2u, 11u, 7u, 5u, 3u
);

fn g(v: ptr<function, array<vec2<u32>, 16>>, a: u32, b: u32, c: u32, d: u32, x: vec2<u32>, y: vec2<u32>) {
    (*v)[a] = add64(add64((*v)[a], (*v)[b]), x);
    (*v)[d] = rotr64((*v)[d] ^ (*v)[a], 32u);
    (*v)[c] = add64((*v)[c], (*v)[d]);
    (*v)[b] = rotr64((*v)[b] ^ (*v)[c], 24u);
    (*v)[a] = add64(add64((*v)[a], (*v)[b]), y);
    (*v)[d] = rotr64((*v)[d] ^ (*v)[a], 16u);
    (*v)[c] = add64((*v)[c], (*v)[d]);
    (*v)[b] = rotr64((*v)[b] ^ (*v)[c], 63u);
}

fn digest(m: ptr<function, array<u32, 16>>, out: ptr<function, array<u32, 8>>) {
    var w: array<vec2<u32>, 16>;
    var h: array<vec2<u32>, 8>;
    var v: array<vec2<u32>, 16>;

    for (var i = 0u; i < 6u; i++) {
        w[i] = vec2<u32>((*m)[2u * i], (*m)[2u * i + 1u]);
    }

    w[6] = vec2<u32>((*m)[12], 0u);

    for (var i = 7u; i < 16u; i++) {
        w[i] = vec2<u32>(0u, 0u);
    }

    for (var i = 0u; i < 8u; i++) {
        h[i] = IV[i];
    }

    // The parameter block: a 32-byte digest, fanout 1, depth 1.
    h[0] = h[0] ^ vec2<u32>(0x01010020u, 0u);

    for (var i = 0u; i < 8u; i++) {
        v[i] = h[i];
        v[i + 8u] = IV[i];
    }

    // 52 bytes in, and the last block.
    v[12] = v[12] ^ vec2<u32>(52u, 0u);
    v[14] = ~v[14];

    for (var r = 0u; r < 12u; r++) {
        let s = r * 16u;
        g(&v, 0u, 4u, 8u, 12u, w[SIGMA[s]], w[SIGMA[s + 1u]]);
        g(&v, 1u, 5u, 9u, 13u, w[SIGMA[s + 2u]], w[SIGMA[s + 3u]]);
        g(&v, 2u, 6u, 10u, 14u, w[SIGMA[s + 4u]], w[SIGMA[s + 5u]]);
        g(&v, 3u, 7u, 11u, 15u, w[SIGMA[s + 6u]], w[SIGMA[s + 7u]]);
        g(&v, 0u, 5u, 10u, 15u, w[SIGMA[s + 8u]], w[SIGMA[s + 9u]]);
        g(&v, 1u, 6u, 11u, 12u, w[SIGMA[s + 10u]], w[SIGMA[s + 11u]]);
        g(&v, 2u, 7u, 8u, 13u, w[SIGMA[s + 12u]], w[SIGMA[s + 13u]]);
        g(&v, 3u, 4u, 9u, 14u, w[SIGMA[s + 14u]], w[SIGMA[s + 15u]]);
    }

    for (var i = 0u; i < 4u; i++) {
        let lane = h[i] ^ v[i] ^ v[i + 8u];
        (*out)[2u * i] = bswap(lane.x);
        (*out)[2u * i + 1u] = bswap(lane.y);
    }
}
