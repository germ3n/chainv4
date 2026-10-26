// BLAKE3 of the 52-byte message: one chunk of one block, the root, the first 32 bytes of output.

const IV: array<u32, 8> = array<u32, 8>(
    0x6A09E667u, 0xBB67AE85u, 0x3C6EF372u, 0xA54FF53Au, 0x510E527Fu, 0x9B05688Cu, 0x1F83D9ABu, 0x5BE0CD19u
);

const PERMUTATION: array<u32, 16> = array<u32, 16>(2u, 6u, 3u, 10u, 7u, 0u, 4u, 13u, 1u, 11u, 12u, 5u, 9u, 14u, 15u, 8u);

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
    var v: array<u32, 16>;
    var msg: array<u32, 16>;
    var next: array<u32, 16>;

    for (var i = 0u; i < 16u; i++) {
        msg[i] = (*m)[i];
    }

    for (var i = 0u; i < 8u; i++) {
        v[i] = IV[i];
    }

    v[8] = IV[0];
    v[9] = IV[1];
    v[10] = IV[2];
    v[11] = IV[3];
    // Counter 0, 52 bytes in the block, and the flags: chunk start, chunk end, root.
    v[12] = 0u;
    v[13] = 0u;
    v[14] = 52u;
    v[15] = 11u;

    for (var r = 0u; r < 7u; r++) {
        g(&v, 0u, 4u, 8u, 12u, msg[0], msg[1]);
        g(&v, 1u, 5u, 9u, 13u, msg[2], msg[3]);
        g(&v, 2u, 6u, 10u, 14u, msg[4], msg[5]);
        g(&v, 3u, 7u, 11u, 15u, msg[6], msg[7]);
        g(&v, 0u, 5u, 10u, 15u, msg[8], msg[9]);
        g(&v, 1u, 6u, 11u, 12u, msg[10], msg[11]);
        g(&v, 2u, 7u, 8u, 13u, msg[12], msg[13]);
        g(&v, 3u, 4u, 9u, 14u, msg[14], msg[15]);

        for (var i = 0u; i < 16u; i++) {
            next[i] = msg[PERMUTATION[i]];
        }

        for (var i = 0u; i < 16u; i++) {
            msg[i] = next[i];
        }
    }

    for (var i = 0u; i < 8u; i++) {
        (*out)[i] = bswap(v[i] ^ v[i + 8u]);
    }
}
