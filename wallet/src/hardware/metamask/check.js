// The wallet's codes, read back as MetaMask's QR keyring reads them. In this folder, after npm
// install: CHAINV4_METAMASK_OUT=$PWD/ours.json cargo test --lib metamasks_own, then node check.js.
const { CryptoHDKey, ETHSignature, URRegistryDecoder } = require("@keystonehq/bc-ur-registry-eth");
const { FeeMarketEIP1559Transaction, LegacyTransaction } = require("@ethereumjs/tx");
const { Common } = require("@ethereumjs/common");
const { ethers } = require("ethers");

const requests = require("../metamask.json");
const ours = require("./ours.json");
const CHAIN = 38262171930895n;
const common = Common.custom({ chainId: CHAIN, networkId: CHAIN }, { hardfork: "london" });
const read = (ur) => {
    const decoder = new URRegistryDecoder();
    decoder.receivePart(ur.toLowerCase());
    return decoder.resultRegistryType();
};
let failed = 0;
const check = (what, ok) => {
    console.log(`${ok ? "ok  " : "FAIL"} ${what}`);
    failed += ok ? 0 : 1;
};

// Pairing, as base-eth-keyring's readCryptoHDKey does.
const key = read(ours.pairing);
check("pairing is a crypto-hdkey", key instanceof CryptoHDKey);
const xpub = key.getBip32Key();
const path = `m/${key.getOrigin().getPath()}`;
const xfp = key.getOrigin().getSourceFingerprint().toString("hex");
const children = key.getChildren()?.getPath();
console.log(`     ${path} ${xfp} children ${children} name ${key.getName()} note ${key.getNote()}`);
check("the account level", path === "m/44'/60'/0'");
check("the seed's fingerprint", xfp === requests.xfp);
const node = ethers.HDNodeWallet.fromExtendedKey(xpub);
check("account 0 from the xpub", node.derivePath("0/0").address.toLowerCase() === "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266");
check("account 1 from the xpub", node.derivePath("0/1").address.toLowerCase() === requests.address);

// Signatures, as base-eth-keyring takes them back.
const fee = FeeMarketEIP1559Transaction.fromTxData(
    { nonce: 3, maxPriorityFeePerGas: 1000, maxFeePerGas: 2000000000, gasLimit: 300000, to: "0x3535353535353535353535353535353535353535", value: 10n ** 18n, data: "0x" + "ab".repeat(400) },
    { common },
);
const legacy = LegacyTransaction.fromTxData({ nonce: 4, gasPrice: 2000000000, gasLimit: 21000, to: "0x3535353535353535353535353535353535353535", value: 5n }, { common });

for (const { name, ur } of ours.signatures) {
    // As the keyring does: the UR's CBOR, read as an eth-signature.
    const decoder = new URRegistryDecoder();
    decoder.receivePart(ur.toLowerCase());
    const signature = ETHSignature.fromCBOR(decoder.resultUR().cbor);
    check(`${name}: an eth-signature`, signature instanceof ETHSignature);
    check(`${name}: under the request's id`, signature.getRequestId().toString("hex") === requests.requests[name].id);
    const bytes = signature.getSignature();
    const [r, s, v] = [bytes.subarray(0, 32), bytes.subarray(32, 64), bytes.subarray(64)];
    let from;

    if (name === "fee" || name === "legacy") {
        const tx = name === "fee" ? fee : legacy;
        const json = { ...tx.toJSON(), r: "0x" + r.toString("hex"), s: "0x" + s.toString("hex"), v: "0x" + v.toString("hex"), type: tx.type };
        const signed = name === "fee" ? FeeMarketEIP1559Transaction.fromTxData(json, { common }) : LegacyTransaction.fromTxData(json, { common });
        from = signed.getSenderAddress().toString().toLowerCase();
    } else {
        from = ethers.recoverAddress(requests.requests[name].hash, "0x" + bytes.toString("hex")).toLowerCase();
    }

    check(`${name}: signed by account 1, as MetaMask reads it back`, from === requests.address);
}

process.exit(failed ? 1 : 0);
