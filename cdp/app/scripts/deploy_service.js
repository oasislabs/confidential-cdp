const chalk = require('chalk');
const oasis = require('@oasislabs/client');

oasis.workspace.Cdp.deploy({
  header: {confidential: true},
})
  .then(res => {
    let addrHex = Buffer.from(res._inner.address).toString('hex');
    console.log(`    ${chalk.green('Deployed')} Cdp at 0x${addrHex}`);
  })
  .catch(err => {
    console.error(
      `${chalk.red('error')}: could not deploy Cdp: ${err.message}`,
    );
  })
  .finally(() => {
    oasis.disconnect();
  });
