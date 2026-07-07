# Opaq trusted-setup ceremony transcript

- Date: 2026-07-07T14:25:17Z
- Profile: **smoke**  ⚠️ NOT trustworthy — tooling smoke only

## Phase 1 (reused)
- power 16: `powersOfTau28_hez_final_16.ptau` — blake2b-512 `6a6277a2f74e1073601b4f9fed6e1e55226917efb0f0db8a07d98ab01df1ccf43eb0e8c3159432acd4960e2f29fe84a4198501fa54c8dad9e43297453efec125`
- power 17: `powersOfTau28_hez_final_17.ptau` — blake2b-512 `6247a3433948b35fbfae414fa5a9355bfb45f56efa7ab4929e669264a0258976741dfbe3288bfb49828e5df02c2e633df38d2245e30162ae7e3bcca5b8b49345`
- Source: https://storage.googleapis.com/zkevm/ptau/

## Phase 2: deposit
- Contributions:
  - local-1 → `c_0001.zkey`
  - local-2 → `c_0002.zkey`
- Final beacon: drand:round=6266759 (`01108c93a47c0bd9bd9adfaad4530da266ccdfd6e5d681644c2d763cfa59fc9a`)
- final.zkey blake2b-512: `8b41d8e8a603d630bf8a686eebfa962637f489ac25cb26b0120782e47792e726730c14fab7cedd1c567ebf7dca8f189bb20a67195a393874dbe38c8092ba9647`
- embedded VK (`programs/opaq/src/vk_deposit.rs`) blake2b-512: `bbe7cf576e3c81ba04d0b102485b2e33e6583dec101e8212d7d4c3c599a8b42eb21857b0366531b02822348efb87237016aba5516d681c634a436b7be007bfb9`

## Phase 2: withdraw
- Contributions:
  - local-1 → `c_0001.zkey`
  - local-2 → `c_0002.zkey`
- Final beacon: drand:round=6266761 (`8aa183b22ef01421fca5980f87038b02299a6f2651cad7fbd6a92c7115448f94`)
- final.zkey blake2b-512: `d709104963bf3d6954f9eb68b48e7df673432cb21dbb273fc6ecce8c1e36f7423023483b41c2563d257d245f3d9a944687ef75e84ef45ac7f6de20baa20142c8`
- embedded VK (`programs/opaq/src/vk_withdraw.rs`) blake2b-512: `8b9c4b10363f543c223f8c6462dcbbc825eb00b5ce52e2c9be26afffa73119830650297515f762d9d408812a0b433fa098ed1eb79b6f973a7f0ca7edaec5f1bd`

## Phase 2: transfer
- Contributions:
  - local-1 → `c_0001.zkey`
  - local-2 → `c_0002.zkey`
- Final beacon: drand:round=6266766 (`2ad606891e53d55284a120772ebc9d8ee281dc303d7e0e76ee4774daccc5aa20`)
- final.zkey blake2b-512: `5e3ba8128670c05030d8f0ed067c49ab53601d98eb3582ae55ad25e806d4fa307bc200d06d6b82e5533f788d10ba52ded0b3d7709ea673b21dd341d87776dbf3`
- embedded VK (`programs/opaq/src/vk_transfer.rs`) blake2b-512: `d3d35c6ad152c9e03d74c2f1bad3bd87f320fc5251e823ba97b6bddc1d184ba88ab9ec3886392f5ea8c4770de9f2db32c82c2d4fccb067715b2ea57836c783f4`

## Phase 2: burn
- Contributions:
  - local-1 → `c_0001.zkey`
  - local-2 → `c_0002.zkey`
- Final beacon: drand:round=6266771 (`e12085e67ef5915b3480f42afe90667bedc1753f263ed17ddc256ab89ab8809e`)
- final.zkey blake2b-512: `2b032e88028f26ef535288b1c58cd9d8a01b9bcf3fd4a5484359a2a2757114c64a92db10c5e8420b3a816a5807e29d32cf2e6b9b6f698ae4fea127ce8c1c5b9f`
- embedded VK (`programs/opaq/src/vk_burn.rs`) blake2b-512: `ee63ab43cf40b89873e5f9f6c4af7a98c051d57c2c969753a0947cf60243029cfb8af073b96d95d6cefc4d61d8d95351dfe83eae74382c62a1115b23cd20c256`

## Phase 2: xburn
- Contributions:
  - local-1 → `c_0001.zkey`
  - local-2 → `c_0002.zkey`
- Final beacon: drand:round=6266774 (`eef0af9cf92a0379af3d65c994b71c3e569a0239a7a7a90ad8a9e0f9f7aa7b26`)
- final.zkey blake2b-512: `8b07d9ed0441ed735139b8e319eaf9602757524f79212915e9b689139c7bd7b81fb68a32324fe00961d74f95fd629dc3706c821cdafff3567efd72b550c4a4b5`
- embedded VK (`programs/opaq/src/vk_xburn.rs`) blake2b-512: `e08297e198dcab41488f97d714a5c54b1aff5b67d23601f8365e538444670c977a5e85ced1c0b139cf9b63fb718c0134076facb02a20ea4d0556f2f2c3426894`

## Verify
```
scripts/ceremony-verify.sh deposit:/Users/sergey/Temp/opaq/ceremony/work/deposit withdraw:/Users/sergey/Temp/opaq/ceremony/work/withdraw transfer:/Users/sergey/Temp/opaq/ceremony/work/transfer burn:/Users/sergey/Temp/opaq/ceremony/work/burn xburn:/Users/sergey/Temp/opaq/ceremony/work/xburn
```
