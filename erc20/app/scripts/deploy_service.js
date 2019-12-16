const chalk = require('chalk');
const oasis = require('@oasislabs/client');

const INIT_SUPPLY = 100000;

oasis.workspace.Erc20Token.deploy(INIT_SUPPLY, {
    header: {confidential: false},
    gasLimit: '0xf42400',
})
  .then(res => {
    let addrHex = Buffer.from(res._inner.address).toString('hex');
    console.log(`    ${chalk.green('Deployed')} Erc20 at 0x${addrHex}`);
  })
  .catch(err => {
    console.error(
      `${chalk.red('error')}: could not deploy Erc20: ${err.message}`,
    );
  })
  .finally(() => {
    oasis.disconnect();
  });
