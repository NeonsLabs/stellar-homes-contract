# Testnet smoke run

One mortgage taken through its whole life on the testnet deployment recorded in
[testnet.json](testnet.json), run on 2026-11-09 with `scripts/smoke-testnet.sh`.
Settlement asset: Circle testnet USDC. Amounts: pool deposit 15 USDC, valuation
20 USDC, principal 10 USDC.

| Step | Caller | Transaction |
|---|---|---|
| Investor funds the pool | investor | [c6f005be…](https://stellar.expert/explorer/testnet/tx/c6f005bed2ca411c9489e0b4b0e51239567df8e24ec4036be66b83cf48a5c62b) |
| Trustee registers property 1 | trustee | [0f05c4a9…](https://stellar.expert/explorer/testnet/tx/0f05c4a928ae86f31cf530274f2552aad6ea190600bd5dda8d14359924fa8073) |
| Oracle verifies the title | oracle | [b1fe08b2…](https://stellar.expert/explorer/testnet/tx/b1fe08b2d89f1183f7a26c4dccc78a28296ce2bd7d0db5975ff5a1b2c96d5d60) |
| Oracle publishes the valuation | oracle | [aeb9304e…](https://stellar.expert/explorer/testnet/tx/aeb9304efabcbec6310d5ece31cb08eb5bc4d0ee867f304b98567d29a06ae57e) |
| Borrower applies (mortgage 1) | borrower | [62e71bf1…](https://stellar.expert/explorer/testnet/tx/62e71bf13052bbc38cde1e7ef372df84c4e867639faa530ee896a72bdd4f70a5) |
| Underwriter approves | admin | [6e3a978c…](https://stellar.expert/explorer/testnet/tx/6e3a978cc0395ca5e9910653ac32bc21ac4f3674bdda89ca12289d5b73e31b3d) |
| Trustee submits foundation evidence | trustee | [3eeafdce…](https://stellar.expert/explorer/testnet/tx/3eeafdceb2bad10db018ce644f0d80abb147c0b05fb0ffe84a44db997158541f) |
| Oracle signs off the foundation | oracle | [f1e9c347…](https://stellar.expert/explorer/testnet/tx/f1e9c347a0bf2c9765ca9bd39c4d6e9394c7fe6a88e5d5d87cb3d814d0735c49) |
| Tranche released to the trustee (2 USDC) | admin | [06a6b1ef…](https://stellar.expert/explorer/testnet/tx/06a6b1efc923ba5ab71b1518f439ca067f7338b96fd5c1d344ab2e790f16845d) |
| Borrower repays the 2 USDC drawn | borrower | [15d82e91…](https://stellar.expert/explorer/testnet/tx/15d82e9108385f08b83e4642b15da2be31a3b59a309e594184f1d469010258c4) |

Property 1 ends in status `Repaid`.

## What this run does not show

- The release first failed with a missing trustline on the trustee, then
  succeeded once one was added. The script now opens it when `USDC_ISSUER` is set.
- Only the foundation stage (stage 0) was released, and the loan was cleared
  straight after, so interest was 0. A multi-instalment repayment, an overdue
  loan and a default have not been run on testnet. They are covered by the
  contract test suites.
- The contracts are unaudited and the admin is still a single key.
