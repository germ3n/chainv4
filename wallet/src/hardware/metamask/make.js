// Requests as MetaMask's QR keyring makes them, with Keystone's registry: what
// wallet/src/hardware/metamask.json holds. In this folder: npm install, then
// node make.js > ../metamask.json
const { EthSignRequest, DataType } = require("@keystonehq/bc-ur-registry-eth");
const { FeeMarketEIP1559Transaction, LegacyTransaction } = require("@ethereumjs/tx");
const { Common } = require("@ethereumjs/common");
const { ethers } = require("ethers");
const { v4: uuid, parse: uuidParse } = require("uuid");

const PHRASE = "test test test test test test test test test test test junk";
const CHAIN = 38262171930895n;
const root = ethers.HDNodeWallet.fromPhrase(PHRASE, "", "m");
const xfp = root.fingerprint.slice(2);
const account = root.derivePath("m/44'/60'/0'/0/1");
const common = Common.custom({ chainId: CHAIN, networkId: CHAIN }, { hardfork: "london" });

function request(data, type, id, maxFragment) {
    const req = EthSignRequest.constructETHRequest(data, type, "m/44'/60'/0'/0/1", xfp, id, Number(CHAIN), account.address, "metamask");
    const encoder = req.toUREncoder(maxFragment);
    const parts = [];

    for (let i = 0; i < Math.max(encoder.fragmentsLength * 3, 1); i++) {
        parts.push(encoder.nextPart());
    }

    return { id: Buffer.from(uuidParse(id)).toString("hex"), parts };
}

const fee = FeeMarketEIP1559Transaction.fromTxData(
    { nonce: 3, maxPriorityFeePerGas: 1000, maxFeePerGas: 2000000000, gasLimit: 300000, to: "0x3535353535353535353535353535353535353535", value: 10n ** 18n, data: "0x" + "ab".repeat(400) },
    { common },
);
const legacy = LegacyTransaction.fromTxData({ nonce: 4, gasPrice: 2000000000, gasLimit: 21000, to: "0x3535353535353535353535353535353535353535", value: 5n }, { common });
const typed = {
    types: { EIP712Domain: [{ name: "name", type: "string" }, { name: "chainId", type: "uint256" }], Note: [{ name: "text", type: "string" }] },
    primaryType: "Note",
    domain: { name: "chainv4", chainId: Number(CHAIN) },
    message: { text: "hi" },
};
const message = Buffer.from("Sign in to chainv4");

const out = {
    xfp,
    address: account.address.toLowerCase(),
    requests: {
        fee: { ...request(Buffer.from(fee.getMessageToSign()), DataType.typedTransaction, uuid(), 60), hash: "0x" + Buffer.from(fee.getHashedMessageToSign()).toString("hex") },
        legacy: { ...request(Buffer.from(require("@ethereumjs/rlp").RLP.encode(legacy.getMessageToSign())), DataType.transaction, uuid(), 400), hash: "0x" + Buffer.from(legacy.getHashedMessageToSign()).toString("hex") },
        typed: { ...request(Buffer.from(JSON.stringify(typed)), DataType.typedData, uuid(), 400), hash: ethers.TypedDataEncoder.hash(typed.domain, { Note: typed.types.Note }, typed.message) },
        message: { ...request(message, DataType.personalMessage, uuid(), 400), hash: ethers.hashMessage(message) },
    },
};

console.log(JSON.stringify(out, null, 1));
