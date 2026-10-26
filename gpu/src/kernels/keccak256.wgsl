// Keccak-256 of the 52-byte message, as Ethereum hashes: one 136-byte block, padded 0x01 … 0x80.

const RC: array<vec2<u32>, 24> = array<vec2<u32>, 24>(
    vec2<u32>(0x00000001u, 0x00000000u), vec2<u32>(0x00008082u, 0x00000000u),
    vec2<u32>(0x0000808au, 0x80000000u), vec2<u32>(0x80008000u, 0x80000000u),
    vec2<u32>(0x0000808bu, 0x00000000u), vec2<u32>(0x80000001u, 0x00000000u),
    vec2<u32>(0x80008081u, 0x80000000u), vec2<u32>(0x00008009u, 0x80000000u),
    vec2<u32>(0x0000008au, 0x00000000u), vec2<u32>(0x00000088u, 0x00000000u),
    vec2<u32>(0x80008009u, 0x00000000u), vec2<u32>(0x8000000au, 0x00000000u),
    vec2<u32>(0x8000808bu, 0x00000000u), vec2<u32>(0x0000008bu, 0x80000000u),
    vec2<u32>(0x00008089u, 0x80000000u), vec2<u32>(0x00008003u, 0x80000000u),
    vec2<u32>(0x00008002u, 0x80000000u), vec2<u32>(0x00000080u, 0x80000000u),
    vec2<u32>(0x0000800au, 0x00000000u), vec2<u32>(0x8000000au, 0x80000000u),
    vec2<u32>(0x80008081u, 0x80000000u), vec2<u32>(0x00008080u, 0x80000000u),
    vec2<u32>(0x80000001u, 0x00000000u), vec2<u32>(0x80008008u, 0x80000000u)
);

// Rotation of each lane, by x + 5y.
const ROT: array<u32, 25> = array<u32, 25>(
    0u, 1u, 62u, 28u, 27u,
    36u, 44u, 6u, 55u, 20u,
    3u, 10u, 43u, 25u, 39u,
    41u, 45u, 15u, 21u, 8u,
    18u, 2u, 61u, 56u, 14u
);

fn digest(m: ptr<function, array<u32, 16>>, out: ptr<function, array<u32, 8>>) {
    var a: array<vec2<u32>, 25>;
    var b: array<vec2<u32>, 25>;
    var c: array<vec2<u32>, 5>;
    var d: array<vec2<u32>, 5>;

    for (var i = 0u; i < 25u; i++) {
        a[i] = vec2<u32>(0u, 0u);
    }

    for (var i = 0u; i < 6u; i++) {
        a[i] = vec2<u32>((*m)[2u * i], (*m)[2u * i + 1u]);
    }

    // The last four bytes of the message, then the padding's first byte, 0x01, and its last,
    // 0x80, at the end of the 136-byte block.
    a[6] = vec2<u32>((*m)[12], 0x01u);
    a[16] = vec2<u32>(0u, 0x80000000u);

    for (var rnd = 0u; rnd < 24u; rnd++) {
        for (var x = 0u; x < 5u; x++) {
            c[x] = a[x] ^ a[x + 5u] ^ a[x + 10u] ^ a[x + 15u] ^ a[x + 20u];
        }

        for (var x = 0u; x < 5u; x++) {
            d[x] = c[(x + 4u) % 5u] ^ rotl64(c[(x + 1u) % 5u], 1u);
        }

        for (var i = 0u; i < 25u; i++) {
            a[i] = a[i] ^ d[i % 5u];
        }

        for (var x = 0u; x < 5u; x++) {
            for (var y = 0u; y < 5u; y++) {
                let i = x + 5u * y;
                b[y + 5u * ((2u * x + 3u * y) % 5u)] = rotl64(a[i], ROT[i]);
            }
        }

        for (var x = 0u; x < 5u; x++) {
            for (var y = 0u; y < 5u; y++) {
                let i = x + 5u * y;
                a[i] = b[i] ^ (~b[(x + 1u) % 5u + 5u * y] & b[(x + 2u) % 5u + 5u * y]);
            }
        }

        a[0] = a[0] ^ RC[rnd];
    }

    for (var i = 0u; i < 4u; i++) {
        (*out)[2u * i] = bswap(a[i].x);
        (*out)[2u * i + 1u] = bswap(a[i].y);
    }
}
