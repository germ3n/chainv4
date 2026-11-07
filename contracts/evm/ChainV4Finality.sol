// SPDX-License-Identifier: MIT
// chainv4's finality, followed on an EVM chain. chainv4's validators each name an attester, a
// secp256k1 key apart from their voting key, and beside each vote sign a keccak256 commitment of
// the link they vote for: its source and target checkpoints, the target's height and state root,
// and the hashes of the attesting sets of the target's epoch and the next. This contract takes a
// link once attesters holding two thirds of the set's whole weight signed it, as chainv4 justifies
// a checkpoint: it records the target as justified, with its state root, and the source as final
// when the target is the very next epoch, as Casper FFG finalizes. Then the set it checks the next
// link by is the next epoch's, which the signers committed to, so it follows the set as it changes.
//
// It trusts what two thirds of chainv4's validators sign, as chainv4's own light clients do: what
// it adds is that nothing else is trusted, no federation and no relayer. Anyone may submit a link.
//
// Built with: solc --evm-version prague --optimize --bin --abi ChainV4Finality.sol
pragma solidity ^0.8.24;

contract ChainV4Finality {
    /// What sets an attestation apart from anything else an attester's key signs.
    bytes32 public constant DOMAIN = keccak256("chainv4 attestation");

    /// secp256k1's order halved: an s above it is the other shape of a signature, refused so that no
    /// attestation has two.
    uint256 private constant HALF_ORDER = 0x7fffffffffffffffffffffffffffffff5d576e7357a4501ddfe92f46681b20a0;

    /// The chain followed, by its EVM chain id, as attestations name it.
    uint256 public immutable chainId;

    /// The hash of the attesting set that signs the next link: keccak256(abi.encode(total,
    /// attesters, weights)), attesters by address ascending.
    bytes32 public setHash;

    /// The newest justified checkpoint taken, and the newest final one.
    uint256 public justifiedEpoch;
    bytes32 public justifiedBlock;
    uint256 public finalizedEpoch;
    bytes32 public finalizedBlock;

    /// Each justified checkpoint taken, by epoch: its block, height and state root.
    mapping(uint256 => bytes32) public blockAt;
    mapping(uint256 => uint256) public heightAt;
    mapping(uint256 => bytes32) public stateRootAt;

    event Justified(uint256 indexed epoch, bytes32 block_, uint256 height, bytes32 stateRoot);
    event Finalized(uint256 indexed epoch, bytes32 block_);
    event SetMoved(bytes32 setHash);

    /// Starts from a checkpoint the deployer trusts as final, and the hash of the attesting set
    /// that signs the next link: what a chainv4 node it trusts says.
    constructor(uint256 chainId_, uint256 epoch, bytes32 block_, uint256 height, bytes32 stateRoot, bytes32 setHash_) {
        chainId = chainId_;
        setHash = setHash_;
        justifiedEpoch = epoch;
        justifiedBlock = block_;
        finalizedEpoch = epoch;
        finalizedBlock = block_;
        blockAt[epoch] = block_;
        heightAt[epoch] = height;
        stateRootAt[epoch] = stateRoot;
    }

    /// The digest attesters sign for a link: keccak256(abi.encode(DOMAIN, chainId, sourceEpoch,
    /// sourceBlock, targetEpoch, targetBlock, height, stateRoot, set, nextSet)).
    function digest(bytes32[6] calldata link, bytes32 set, bytes32 nextSet) public view returns (bytes32) {
        return keccak256(abi.encode(DOMAIN, chainId, link[0], link[1], link[2], link[3], link[4], link[5], set, nextSet));
    }

    /// Takes a link once enough of the attesting set signed it.
    ///   link: sourceEpoch, sourceBlock, targetEpoch, targetBlock, the target's height and state
    ///         root, each a word.
    ///   total, attesters, weights: the attesting set that signs it, whose hash is setHash.
    ///   nextSet: the hash of the next epoch's attesting set, which it moves to.
    ///   signers: the places in attesters of those that signed, ascending.
    ///   signatures: their attestations, 65 bytes each, r, s and v, in the same order.
    function submit(
        bytes32[6] calldata link,
        uint256 total,
        address[] calldata attesters,
        uint256[] calldata weights,
        bytes32 nextSet,
        uint256[] calldata signers,
        bytes calldata signatures
    ) external {
        require(attesters.length == weights.length, "one weight for each attester");
        require(keccak256(abi.encode(total, attesters, weights)) == setHash, "not the set that signs the next link");
        require(link[1] != bytes32(0) && blockAt[uint256(link[0])] == link[1], "its source is no justified checkpoint here");
        require(uint256(link[2]) > uint256(link[0]) && uint256(link[2]) > justifiedEpoch, "its target is no later checkpoint");
        require(signatures.length == signers.length * 65, "65 bytes for each signer");
        require(signed(digest(link, setHash, nextSet), attesters, weights, signers, signatures) * 3 >= total * 2, "short of two thirds of the set");

        record(link, nextSet);
    }

    /// The weight of the attesters whose attestations of a digest these are.
    function signed(bytes32 linked, address[] calldata attesters, uint256[] calldata weights, uint256[] calldata signers, bytes calldata signatures) private pure returns (uint256 weight) {
        for (uint256 i = 0; i < signers.length; i++) {
            uint256 place = signers[i];
            require(place < attesters.length && (i == 0 || place > signers[i - 1]), "signers ascending, each once");
            require(recover(linked, signatures[i * 65:(i + 1) * 65]) == attesters[place], "an attestation is not its attester's");
            weight += weights[place];
        }
    }

    /// Takes a link that holds: its target justified, its source final when the target is the
    /// very next epoch, and the next epoch's set as the one that signs from now on.
    function record(bytes32[6] calldata link, bytes32 nextSet) private {
        uint256 sourceEpoch = uint256(link[0]);
        uint256 targetEpoch = uint256(link[2]);
        justifiedEpoch = targetEpoch;
        justifiedBlock = link[3];
        blockAt[targetEpoch] = link[3];
        heightAt[targetEpoch] = uint256(link[4]);
        stateRootAt[targetEpoch] = link[5];
        emit Justified(targetEpoch, link[3], uint256(link[4]), link[5]);

        if (targetEpoch == sourceEpoch + 1 && sourceEpoch > finalizedEpoch) {
            finalizedEpoch = sourceEpoch;
            finalizedBlock = link[1];
            emit Finalized(sourceEpoch, link[1]);
        }

        if (nextSet != setHash) {
            setHash = nextSet;
            emit SetMoved(nextSet);
        }
    }

    function recover(bytes32 linked, bytes calldata signature) private pure returns (address) {
        bytes32 r = bytes32(signature[0:32]);
        bytes32 s = bytes32(signature[32:64]);
        uint8 v = uint8(signature[64]);
        require(uint256(s) <= HALF_ORDER && (v == 27 || v == 28), "an attestation not in its one shape");
        address recovered = ecrecover(linked, v, r, s);
        require(recovered != address(0), "an attestation that recovers no key");

        return recovered;
    }
}
