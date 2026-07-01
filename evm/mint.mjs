// Self-served cross-chain MINT (OPAQ.md A.6, EVM side): given a real burn proof
// (snarkjs public.json + proof.json), format it as OpaqMint.mint(a,b,c,signals)
// calldata and submit the tx — NO relayer, the note owner mints for themselves.
// Uses snarkjs (`export soliditycalldata`, which already orders the G2 coords for
// the pairing) + Foundry `cast` for the transaction.
//
// Usage: node mint.mjs <rpc> <opaqMintAddr> <privKey> <public.json> <proof.json>
import { execFileSync } from "node:child_process";

const [rpc, addr, key, pub, proof] = process.argv.slice(2);
if (!rpc || !addr || !key || !pub || !proof) {
  console.error("usage: node mint.mjs <rpc> <opaqMintAddr> <privKey> <public.json> <proof.json>");
  process.exit(2);
}

// cast wants array literals as `[e0,e1]` (nested for uint[2][2]); the snarkjs
// calldata elements are already 0x-hex, so just bracket-join them.
const arr = (a) => `[${a.join(",")}]`;
const arr2 = (b) => `[${b.map(arr).join(",")}]`;

try {
  const calldata = execFileSync(
    "snarkjs", ["zkey", "export", "soliditycalldata", pub, proof], { encoding: "utf8" },
  ).trim();
  const [a, b, c, sig] = JSON.parse("[" + calldata + "]");
  const out = execFileSync("cast", [
    "send", addr,
    "mint(uint256[2],uint256[2][2],uint256[2],uint256[6])",
    arr(a), arr2(b), arr(c), arr(sig),
    "--rpc-url", rpc, "--private-key", key, "--json",
  ], { encoding: "utf8" });
  process.stdout.write((JSON.parse(out).transactionHash) || out.trim());
} catch (e) {
  const msg = e?.stderr?.toString?.() || e?.stdout?.toString?.() || e?.message || String(e);
  console.error(`mint: ${msg}`);
  process.exit(1);
}
