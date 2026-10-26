// 64-bit words as pairs of 32-bit ones, low half first, for the hashes built on them.

fn add64(a: vec2<u32>, b: vec2<u32>) -> vec2<u32> {
    let lo = a.x + b.x;
    let carry = select(0u, 1u, lo < a.x);

    return vec2<u32>(lo, a.y + b.y + carry);
}

fn rotl64(x: vec2<u32>, n: u32) -> vec2<u32> {
    if (n == 0u) {
        return x;
    }

    if (n == 32u) {
        return vec2<u32>(x.y, x.x);
    }

    if (n < 32u) {
        return vec2<u32>((x.x << n) | (x.y >> (32u - n)), (x.y << n) | (x.x >> (32u - n)));
    }

    let k = n - 32u;

    return vec2<u32>((x.y << k) | (x.x >> (32u - k)), (x.x << k) | (x.y >> (32u - k)));
}

fn rotr64(x: vec2<u32>, n: u32) -> vec2<u32> {
    return rotl64(x, 64u - n);
}
