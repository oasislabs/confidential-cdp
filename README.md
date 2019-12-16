# Private Money Market using Collateralized Debt Position (CDP)
NOTE: Do NOT make public until code review

## Quick Overview

This is a money market contract that uses collateralized debt position (CDP) to let users lend and borrow money. It pools tokens from investors and lends them off to borrowers. The borrowers need to first overcollateralize their loan (collateral > loan) to account for volatility in crypto prices. The borrowers pay interest for their loan which is then collected and distributed to investors.

## Deploying the contract

First deploy an ERC20 token. Then deploy the money market contract.
```
$ cd erc20/service
$ oasis build
$ cd ../app
$ oasis deploy
$ cd ../../cdp/service
$ oasis build
$ cd ../app
$ oasis deploy
```

## Client interaction

Client code can be written in Javascript to interact with the contract. See sample client-side code the `app` folders of `erc20` and `cdp`.
