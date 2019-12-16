import oasis from '@oasislabs/client';

jest.setTimeout(20000);
const GAS_LIMIT = '0xf42400';
const options = { gasLimit: GAS_LIMIT };

describe('Erc20', () => {
  let service;

  beforeAll(async () => {
    service = await oasis.workspace.Erc20Token.deploy(2000, {
      header: {confidential: false},
    });
  });

  it('deployed', async () => {
    expect(service).toBeTruthy();
    console.log(service);
  });

  afterAll(() => {
    oasis.disconnect();
  });
});
