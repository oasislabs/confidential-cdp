import oasis from '@oasislabs/client';

jest.setTimeout(20000);
const GAS_LIMIT = '0xf42400';
const options = { gasLimit: GAS_LIMIT };


describe('Cdp', () => {
    let service;
    beforeAll(async () => {
        service = await oasis.workspace.Cdp.deploy({
            header: {confidential: true},
        });
    });

    it('deployed', async () => {
        expect(service).toBeTruthy();
        let addr = await service.cdpAddr(options);
        let admins = await service.listAdmin(options);
    });

    it('add ERC20A market', async () => {
        const aName = "oERC20_A";
        const aAddr =  [248,180,118,134,45,212,188,170,171,185,136,170,90,69,157,149,227,25,72,14];
        const aPrice = 350.0;
        await service.addMarket(aName, aPrice, aAddr, options);
        
        const listed = await service.mmListed(aName, options);
        expect(listed).toEqual(true);

        let cdp = await service.showAll(options);
    });

  afterAll(() => {
    oasis.disconnect();
  });
});
