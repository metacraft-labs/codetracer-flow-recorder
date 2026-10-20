import OffersV2 from 0xb8ea91944fd51c43
import DapperOffersV2 from 0xb8ea91944fd51c43
import TopShot from 0x0b2a3299cc857e29
import NonFungibleToken from 0x1d7e57aa55817448
import FungibleToken from 0xf233dcee88fe0abe
import DapperUtilityCoin from 0xead892083b3e2c6c
import Resolver from 0xb8ea91944fd51c43
import MetadataViews from 0x1d7e57aa55817448

transaction(amount: UFix64, royalties: {Address:UFix64}, offerParamsString: {String:String}) {
    var nftReceiver: Capability<&{NonFungibleToken.CollectionPublic}>?
    let dapperOfferRef: auth(DapperOffersV2.Manager) &DapperOffersV2.DapperOffer
    var ducWithdrawCap: Capability<auth(FungibleToken.Withdraw) &{FungibleToken.Vault}>
    let resolverCap: Capability<&{Resolver.ResolverPublic}>
    var proxyManagerCap: Capability<auth(DapperOffersV2.ProxyManager) &DapperOffersV2.DapperOffer>

    prepare(signer: auth(Storage, Capabilities) &Account, dapper: auth(Storage, Capabilities) &Account) {
        let collectionData = TopShot.resolveContractView(resourceType: nil, viewType: Type<MetadataViews.NFTCollectionData>())! as! MetadataViews.NFTCollectionData

        if signer.storage.borrow<&{NonFungibleToken.Collection}>(from: collectionData.storagePath) == nil {
            signer.storage.save(<- collectionData.createEmptyCollection(), to: collectionData.storagePath)
            signer.capabilities.unpublish(collectionData.publicPath)
            signer.capabilities.publish(
                signer.capabilities.storage.issue<&{NonFungibleToken.CollectionPublic}>(collectionData.storagePath),
                at: collectionData.publicPath
            )
        }

        self.nftReceiver = signer.capabilities.get<&{NonFungibleToken.CollectionPublic}>(collectionData.publicPath)
        if self.nftReceiver == nil || !self.nftReceiver!.check() {
            signer.capabilities.unpublish(collectionData.publicPath)
            signer.capabilities.publish(
                signer.capabilities.storage.issue<&{NonFungibleToken.CollectionPublic}>(collectionData.storagePath),
                at: collectionData.publicPath
            )
            self.nftReceiver = signer.capabilities.get<&{NonFungibleToken.CollectionPublic}>(collectionData.publicPath)
        }

        if signer.storage.borrow<&DapperOffersV2.DapperOffer>(from: DapperOffersV2.DapperOffersStoragePath) == nil {
            signer.storage.save(<- DapperOffersV2.createDapperOffer(), to: DapperOffersV2.DapperOffersStoragePath)
            signer.capabilities.unpublish(DapperOffersV2.DapperOffersPublicPath)
            signer.capabilities.publish(
                signer.capabilities.storage.issue<&DapperOffersV2.DapperOffer>(DapperOffersV2.DapperOffersStoragePath),
                at: DapperOffersV2.DapperOffersPublicPath
            )
        }

        if dapper.storage.borrow<&DapperOffersV2.DapperOffer>(from: DapperOffersV2.DapperOffersStoragePath) == nil {
            signer.storage.save(<- DapperOffersV2.createDapperOffer(), to: DapperOffersV2.DapperOffersStoragePath)
            signer.capabilities.publish(
                dapper.capabilities.storage.issue<&DapperOffersV2.DapperOffer>(DapperOffersV2.DapperOffersStoragePath),
                at: DapperOffersV2.DapperOffersPublicPath
            )
        }

        self.dapperOfferRef = signer.storage.borrow<auth(DapperOffersV2.Manager) &DapperOffersV2.DapperOffer>(from: DapperOffersV2.DapperOffersStoragePath)
            ?? panic("Could not borrow DapperOffersV2.DapperOffer")

        let dapperOfferProxyManagerCapPath = /storage/DapperOfferProxyManagerCapability
        var dapperOfferProxyManagerCap = signer.storage.copy<Capability<auth(DapperOffersV2.ProxyManager) &DapperOffersV2.DapperOffer>>(from: dapperOfferProxyManagerCapPath)
        if dapperOfferProxyManagerCap == nil || !dapperOfferProxyManagerCap!.check() {
            self.proxyManagerCap = signer.capabilities.storage.issue<auth(DapperOffersV2.ProxyManager) &DapperOffersV2.DapperOffer>(DapperOffersV2.DapperOffersStoragePath)
            signer.capabilities.storage.getController(byCapabilityID: self.proxyManagerCap.id)!.setTag("DapperOfferProxyManagerCap")
            signer.storage.save(
                self.proxyManagerCap,
                to: dapperOfferProxyManagerCapPath
            )
        } else {
            self.proxyManagerCap = dapperOfferProxyManagerCap!
        }

        dapper.capabilities.borrow<&DapperOffersV2.DapperOffer>(DapperOffersV2.DapperOffersPublicPath)!.addProxyCapability(
            account: signer.address,
            cap: self.proxyManagerCap,
        )

        let ducWithdrawCapPath = /storage/DUCWithdrawCapabilityForOffers
        var ducWithdrawCap = dapper.storage.copy<Capability<auth(FungibleToken.Withdraw) &{FungibleToken.Vault}>>(from: ducWithdrawCapPath)
        if ducWithdrawCap == nil || !ducWithdrawCap!.check() {
            self.ducWithdrawCap = dapper.capabilities.storage.issue<auth(FungibleToken.Withdraw) &{FungibleToken.Vault}>(/storage/dapperUtilityCoinVault)
            dapper.capabilities.storage.getController(byCapabilityID: self.ducWithdrawCap.id)!.setTag("OffersDUCWithdrawCap")
            dapper.storage.save(
                self.ducWithdrawCap,
                to: ducWithdrawCapPath
            )
        } else {
            self.ducWithdrawCap = ducWithdrawCap!
        }

        if signer.storage.borrow<&Resolver.OfferResolver>(from: /storage/OfferResolver) == nil {
            signer.storage.save(<- Resolver.createResolver(), to: /storage/OfferResolver)
            signer.capabilities.unpublish(/public/OfferResolver)
            signer.capabilities.publish(
                signer.capabilities.storage.issue<&Resolver.OfferResolver>(/storage/OfferResolver),
                at: /public/OfferResolver
            )
        }

        self.resolverCap = signer.capabilities.get<&{Resolver.ResolverPublic}>(/public/OfferResolver)
    }

    pre {
        (royalties[0xfaf0cc52c6e3acaf] == 0.05000000 * amount): "Missing or mis-typed royalty information"
        (royalties.keys.length == 1): "please provide only one royalty address"
    }

    execute {
        var royaltysList: [OffersV2.Royalty] = []
        for key in royalties.keys {
            royaltysList.append(OffersV2.Royalty(
                receiver: getAccount(key).capabilities.get<&{FungibleToken.Receiver}>(/public/dapperUtilityCoinReceiver)!,
                amount: royalties[key]!
            ))
        }

        offerParamsString.insert(key: "typeId", "Type<@TopShot.NFT>()")

        if offerParamsString.containsKey("nftId") {
            offerParamsString.insert(key: "_type", "NFT")
            offerParamsString.insert(key: "resolver", Resolver.ResolverType.NFT.rawValue.toString())
        } else if offerParamsString.containsKey("playId") && offerParamsString.containsKey("setId") {
            if offerParamsString.containsKey("subeditionId") {
                offerParamsString.insert(key: "_type", "TopShotSubedition")
            } else {
                offerParamsString.insert(key: "_type", "TopShotEdition")
            }
            offerParamsString.insert(key: "resolver", Resolver.ResolverType.TopShotEdition.rawValue.toString())
        } else if offerParamsString.containsKey("editionId") {
            if offerParamsString.containsKey("serialNumber") {
                offerParamsString.insert(key: "_type", "NFT")
            } else {
                offerParamsString.insert(key: "_type", "EDITION")
            }
            offerParamsString.insert(key: "resolver", Resolver.ResolverType.EditionIdAndSerialNumberTraits.rawValue.toString())
        }

        self.dapperOfferRef.createOffer(
            vaultRefCapability: self.ducWithdrawCap,
            nftReceiverCapability: self.nftReceiver!,
            nftType: Type<@TopShot.NFT>(),
            amount: amount,
            royalties: royaltysList,
            offerParamsString: offerParamsString,
            offerParamsUFix64: {},
            offerParamsUInt64: {},
            resolverCapability: self.resolverCap
        )
    }
}
