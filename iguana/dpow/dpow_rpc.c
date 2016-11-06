/******************************************************************************
 * Copyright © 2014-2016 The SuperNET Developers.                             *
 *                                                                            *
 * See the AUTHORS, DEVELOPER-AGREEMENT and LICENSE files at                  *
 * the top-level directory of this distribution for the individual copyright  *
 * holder information and the developer policies on copyright and licensing.  *
 *                                                                            *
 * Unless otherwise agreed in a custom licensing agreement, no part of the    *
 * SuperNET software, including this file may be copied, modified, propagated *
 * or distributed except according to the terms contained in the LICENSE file *
 *                                                                            *
 * Removal or modification of this copyright notice is prohibited.            *
 *                                                                            *
 ******************************************************************************/

int32_t komodo_notaries(uint8_t pubkeys[64][33],int32_t kmdheight)
{
    int32_t i,num=-1; struct iguana_info *coin; char params[256],*retstr,*pubkeystr; cJSON *retjson,*item,*array;
    if ( (coin= iguana_coinfind("KMD")) != 0 )
    {
        if ( coin->FULLNODE < 0 )
        {
            sprintf(params,"[\"%d\"]",kmdheight);
            if ( (retstr= bitcoind_passthru(coin->symbol,coin->chain->serverport,coin->chain->userpass,"notaries",params)) != 0 )
            {
                if ( (retjson= cJSON_Parse(retstr)) != 0 )
                {
                    //printf("%s\n",retstr);
                    if ( (array= jarray(&num,retjson,"notaries")) != 0 )
                    {
                        if ( num > 64 )
                        {
                            printf("warning: numnotaries.%d? > 64?\n",num);
                            num = 64;
                        }
                        for (i=0; i<num; i++)
                        {
                            item = jitem(array,i);
                            if ( (pubkeystr= jstr(item,"pubkey")) != 0 && strlen(pubkeystr) == 66 )
                                decode_hex(pubkeys[i],33,pubkeystr);
                            else printf("error i.%d of %d (%s)\n",i,num,pubkeystr!=0?pubkeystr:"");
                        }
                    }
                    free_json(retjson);
                }
                free(retstr);
            }
        }
    }
    return(num);
}

bits256 dpow_getbestblockhash(struct supernet_info *myinfo,struct iguana_info *coin)
{
    char *retstr; bits256 blockhash;
    if ( 0 && strcmp(coin->symbol,"USD") == 0 )
        printf("dpow_getbestblockhash %s FULLNODE.%d\n",coin->symbol,coin->FULLNODE);
    memset(blockhash.bytes,0,sizeof(blockhash));
    if ( coin->FULLNODE < 0 )
    {
        if ( (retstr= bitcoind_passthru(coin->symbol,coin->chain->serverport,coin->chain->userpass,"getbestblockhash","")) != 0 )
        {
            if ( 0 && strcmp(coin->symbol,"USD") == 0 )
                printf("%s getbestblockhash.(%s)\n",coin->symbol,retstr);
            if ( is_hexstr(retstr,0) == sizeof(blockhash)*2 )
                decode_hex(blockhash.bytes,sizeof(blockhash),retstr);
            free(retstr);
        }
    }
    else if ( coin->FULLNODE > 0 || coin->VALIDATENODE > 0 )
    {
        blockhash = coin->blocks.hwmchain.RO.hash2;
    }
    else
    {
        
    }
    return(blockhash);
}

cJSON *dpow_getblock(struct supernet_info *myinfo,struct iguana_info *coin,bits256 blockhash)
{
    char buf[128],str[65],*retstr=0; cJSON *json = 0;
    if ( coin->FULLNODE < 0 )
    {
        sprintf(buf,"\"%s\"",bits256_str(str,blockhash));
        retstr = bitcoind_passthru(coin->symbol,coin->chain->serverport,coin->chain->userpass,"getblock",buf);
        if ( 0 && strcmp(coin->symbol,"USD") == 0 )
            printf("%s getblock.(%s)\n",coin->symbol,retstr);
    }
    else if ( coin->FULLNODE > 0 || coin->VALIDATENODE > 0 )
    {
        retstr = bitcoinrpc_getblock(myinfo,coin,0,0,blockhash,1,0);
    }
    else
    {
        return(0);
    }
    if ( retstr != 0 )
    {
        json = cJSON_Parse(retstr);
        free(retstr);
    }
    return(json);
}

char *dpow_decoderawtransaction(struct supernet_info *myinfo,struct iguana_info *coin,char *rawtx)
{
    char *retstr,*paramstr; cJSON *array;
    if ( coin->FULLNODE < 0 )
    {
        array = cJSON_CreateArray();
        jaddistr(array,rawtx);
        paramstr = jprint(array,1);
        retstr = bitcoind_passthru(coin->symbol,coin->chain->serverport,coin->chain->userpass,"decoderawtransaction",paramstr);
        //printf("%s decoderawtransaction.(%s) <- (%s)\n",coin->symbol,retstr,paramstr);
        free(paramstr);
    }
    else if ( coin->FULLNODE > 0 || coin->VALIDATENODE > 0 )
    {
        retstr = bitcoinrpc_decoderawtransaction(myinfo,coin,0,0,rawtx,1);
    }
    else
    {
        return(0);
    }
    return(retstr);
}

cJSON *dpow_gettransaction(struct supernet_info *myinfo,struct iguana_info *coin,bits256 txid)
{
    char buf[128],str[65],*retstr=0,*rawtx=0; cJSON *json = 0;
    if ( coin->FULLNODE < 0 )
    {
        sprintf(buf,"[\"%s\", 1]",bits256_str(str,txid));
        if ( (rawtx= bitcoind_passthru(coin->symbol,coin->chain->serverport,coin->chain->userpass,"getrawtransaction",buf)) != 0 )
        {
            retstr = dpow_decoderawtransaction(myinfo,coin,rawtx);
            free(rawtx);
        }
    }
    else if ( coin->FULLNODE > 0 || coin->VALIDATENODE > 0 )
    {
        retstr = bitcoinrpc_getrawtransaction(myinfo,coin,0,0,txid,1);
    }
    else
    {
        return(0);
    }
    if ( retstr != 0 )
    {
        json = cJSON_Parse(retstr);
        free(retstr);
    }
    return(json);
}

cJSON *dpow_listunspent(struct supernet_info *myinfo,struct iguana_info *coin,char *coinaddr)
{
    char buf[128],*retstr; cJSON *json = 0;
    if ( coin->FULLNODE < 0 )
    {
        sprintf(buf,"0, 99999999, [\"%s\"]",coinaddr);
        if ( (retstr= bitcoind_passthru(coin->symbol,coin->chain->serverport,coin->chain->userpass,"listunspent",buf)) != 0 )
        {
            json = cJSON_Parse(retstr);
            //printf("%s (%s) listunspent.(%s)\n",coin->symbol,buf,retstr);
            free(retstr);
        } else printf("%s null retstr from (%s)n",coin->symbol,buf);
    }
    else if ( coin->FULLNODE > 0 || coin->VALIDATENODE > 0 )
    {
        json = iguana_listunspents(myinfo,coin,0,1,coin->longestchain,"");
    }
    else
    {
        return(0);
    }
    return(json);
}

char *dpow_signrawtransaction(struct supernet_info *myinfo,struct iguana_info *coin,char *rawtx,cJSON *vins)
{
    cJSON *array,*privkeys,*item; char *wifstr,*str,*paramstr,*retstr; uint8_t script[256]; int32_t i,n,len,hashtype; struct vin_info V; struct iguana_waddress *waddr; struct iguana_waccount *wacct;
    if ( coin->FULLNODE < 0 )
    {
        array = cJSON_CreateArray();
        jaddistr(array,rawtx);
        jaddi(array,jduplicate(vins));
        paramstr = jprint(array,1);
        retstr = bitcoind_passthru(coin->symbol,coin->chain->serverport,coin->chain->userpass,"signrawtransaction",paramstr);
        //printf("%s signrawtransaction.(%s) params.(%s)\n",coin->symbol,retstr,paramstr);
        free(paramstr);
        return(retstr);
    }
    else if ( coin->FULLNODE > 0 || coin->VALIDATENODE > 0 )
    {
        privkeys = cJSON_CreateArray();
        if ( (n= cJSON_GetArraySize(vins)) > 0 )
        {
            for (i=0; i<n; i++)
            {
                wifstr = "";
                item = jitem(vins,i);
                if ( (str= jstr(item,"scriptPubkey")) != 0 && is_hexstr(str,0) > 0 && strlen(str) < sizeof(script)*2 )
                {
                    len = (int32_t)strlen(str) >> 1;
                    decode_hex(script,len,str);
                    V.spendlen = len;
                    memcpy(V.spendscript,script,len);
                    if ( (hashtype= _iguana_calcrmd160(coin,&V)) >= 0 && V.coinaddr[0] != 0 )
                    {
                        if ( (waddr= iguana_waddresssearch(myinfo,&wacct,V.coinaddr)) != 0 )
                        {
                            if ( bits256_nonz(waddr->privkey) != 0 )
                            {
                                if ( bitcoin_priv2wif(waddr->wifstr,waddr->privkey,coin->chain->wiftype) > 0 )
                                {
                                    wifstr = waddr->wifstr;
                                }
                            }
                        }
                    }
                }
                jaddistr(privkeys,wifstr);
            }
        }
        retstr = bitcoinrpc_signrawtransaction(myinfo,coin,0,0,rawtx,vins,privkeys,"ALL");
        free_json(privkeys);
        return(retstr);
    }
    else
    {
        return(0);
    }
}

char *dpow_sendrawtransaction(struct supernet_info *myinfo,struct iguana_info *coin,char *signedtx)
{
    bits256 txid; cJSON *json,*array; char *paramstr,*retstr;
    if ( coin->FULLNODE < 0 )
    {
        array = cJSON_CreateArray();
        jaddistr(array,signedtx);
        paramstr = jprint(array,1);
        retstr = bitcoind_passthru(coin->symbol,coin->chain->serverport,coin->chain->userpass,"sendrawtransaction",paramstr);
        printf(">>>>>>>>>>> %s sendrawtransaction.(%s) -> %s\n",coin->symbol,paramstr,retstr);
        free(paramstr);
        return(retstr);
    }
    else if ( coin->FULLNODE > 0 || coin->VALIDATENODE > 0 )
    {
        txid = iguana_sendrawtransaction(myinfo,coin,signedtx);
        json = cJSON_CreateObject();
        jaddbits256(json,"result",txid);
        return(jprint(json,1));
    }
    else
    {
        return(0);
    }
}

int32_t dpow_getchaintip(struct supernet_info *myinfo,bits256 *blockhashp,uint32_t *blocktimep,bits256 *txs,uint32_t *numtxp,struct iguana_info *coin)
{
    int32_t n,i,height = -1,maxtx = *numtxp; bits256 besthash; cJSON *array,*json;
    *numtxp = *blocktimep = 0;
    *blockhashp = besthash = dpow_getbestblockhash(myinfo,coin);
    if ( bits256_nonz(besthash) != 0 )
    {
        if ( (json= dpow_getblock(myinfo,coin,besthash)) != 0 )
        {
            if ( (height= juint(json,"height")) != 0 && (*blocktimep= juint(json,"time")) != 0 )
            {
                if ( height > coin->longestchain )
                    coin->longestchain = height;
                if ( (array= jarray(&n,json,"tx")) != 0 )
                {
                    for (i=0; i<n&&i<maxtx; i++)
                        txs[i] = jbits256i(array,i);
                    if ( 0 && strcmp(coin->symbol,"USD") == 0 )
                        printf("dpow_getchaintip %s ht.%d time.%u numtx.%d\n",coin->symbol,height,*blocktimep,n);
                    *numtxp = n;
                }
            } else height = -1;
            free_json(json);
        }
    }
    return(height);
}

int32_t dpow_vini_ismine(struct supernet_info *myinfo,struct dpow_info *dp,cJSON *item)
{
    cJSON *sobj; char *hexstr; int32_t len; uint8_t data[35];
    if ( (sobj= jobj(item,"scriptPubKey")) != 0 && (hexstr= jstr(sobj,"hex")) != 0 )
    {
        len = (int32_t)strlen(hexstr) >> 1;
        if ( len <= sizeof(data) )
        {
            decode_hex(data,len,hexstr);
            if ( len == 35 && data[34] == CHECKSIG && data[0] == 33 && memcmp(data+1,dp->minerkey33,33) == 0 )
                return(0);
        }
    }
    return(-1);
}

int32_t dpow_haveutxo(struct supernet_info *myinfo,struct iguana_info *coin,bits256 *txidp,int32_t *voutp,char *coinaddr)
{
    int32_t i,n,vout,haveutxo = 0; bits256 txid; cJSON *unspents,*item; uint64_t satoshis; char *str,*address; uint8_t script[35];
    memset(txidp,0,sizeof(*txidp));
    *voutp = -1;
    if ( (unspents= dpow_listunspent(myinfo,coin,coinaddr)) != 0 )
    {
        if ( (n= cJSON_GetArraySize(unspents)) > 0 )
        {
            /*{
             "txid" : "34bc21b40d6baf38e2db5be5353dd0bcc9fe416485a2a68753541ed2f9c194b1",
             "vout" : 0,
             "address" : "RFBmvBaRybj9io1UpgWM4pzgufc3E4yza7",
             "scriptPubKey" : "21039a3f7373ae91588b9edd76a9088b2871f62f3438d172b9f18e0581f64887404aac",
             "amount" : 3.00000000,
             "confirmations" : 4282,
             "spendable" : true
             },*/
            for (i=0; i<n; i++)
            {
                item = jitem(unspents,i);
                satoshis = SATOSHIDEN * jdouble(item,"amount");
                if ( satoshis == DPOW_UTXOSIZE && (address= jstr(item,"address")) != 0 && strcmp(address,coinaddr) == 0 )
                {
                    if ( (str= jstr(item,"scriptPubKey")) != 0 && is_hexstr(str,0) == sizeof(script)*2 )
                    {
                        txid = jbits256(item,"txid");
                        vout = jint(item,"vout");
                        if ( bits256_nonz(txid) != 0 && vout >= 0 )
                        {
                            if ( *voutp < 0 || (rand() % (n/2+1)) == 0 )
                            {
                                *voutp = vout;
                                *txidp = txid;
                            }
                            haveutxo++;
                        }
                    }
                }
            }
            if ( haveutxo == 0 )
                printf("no utxo: need to fund address.(%s) or wait for splitfund to confirm\n",coinaddr);
        } else printf("null utxo array size\n");
        free_json(unspents);
    } else printf("null return from dpow_listunspent\n");
    if ( haveutxo > 0 )
        printf("%s haveutxo.%d\n",coin->symbol,haveutxo);
    return(haveutxo);
}

char *dpow_issuemethod(char *userpass,char *method,char *params,uint16_t port)
{
    char url[512],*retstr=0,*retstr2=0,postdata[8192];
    if ( params == 0 || params[0] == 0 )
        params = (char *)"[]";
    if ( strlen(params) < sizeof(postdata)-128 )
    {
        sprintf(url,(char *)"http://127.0.0.1:%u",port);
        sprintf(postdata,"{\"method\":\"%s\",\"params\":%s}",method,params);
        //printf("postdata.(%s) USERPASS.(%s)\n",postdata,KMDUSERPASS);
        retstr2 = bitcoind_RPC(&retstr,(char *)"debug",url,userpass,method,params);
    }
    return(retstr2);
}

uint64_t dpow_paxprice(int32_t height,char *base,char *rel,uint64_t basevolume)
{
    char params[512],*retstr; uint64_t satoshis = 0; cJSON *retjson,*result; struct iguana_info *kmdcoin;
    kmdcoin = iguana_coinfind("KMD");
    sprintf(params,"[\"%s\", \"%s\", \"%d\", \"%.8f\"]",base,rel,height,(double)basevolume/SATOSHIDEN);
    if ( kmdcoin != 0 && (retstr= dpow_issuemethod(kmdcoin->chain->userpass,"paxprice",params,kmdcoin->chain->rpcport)) != 0 )
    {
        if ( (retjson= cJSON_Parse(retstr)) != 0 )
        {
            if ( (result= jobj(retjson,"result")) != 0 )
                satoshis = jdouble(result,"price") * SATOSHIDEN;
            free_json(retjson);
        }
        printf("dpow_paxprice.(%s) -> %s %.8f\n",params,retstr,dstr(satoshis));
    }
    return(satoshis);
}

#define KOMODO_PUBTYPE 60

int32_t PAX_pubkey(int32_t rwflag,uint8_t *pubkey33,uint8_t *addrtypep,uint8_t rmd160[20],char fiat[4],uint8_t *shortflagp,int64_t *fiatoshisp)
{
    if ( rwflag != 0 )
    {
        memset(pubkey33,0,33);
        pubkey33[0] = 0x02 | (*shortflagp != 0);
        memcpy(&pubkey33[1],fiat,3);
        iguana_rwnum(rwflag,&pubkey33[4],sizeof(*fiatoshisp),(void *)fiatoshisp);
        pubkey33[12] = *addrtypep;
        memcpy(&pubkey33[13],rmd160,20);
    }
    else
    {
        *shortflagp = (pubkey33[0] == 0x03);
        memcpy(fiat,&pubkey33[1],3);
        fiat[3] = 0;
        iguana_rwnum(rwflag,&pubkey33[4],sizeof(*fiatoshisp),(void *)fiatoshisp);
        if ( *shortflagp != 0 )
            *fiatoshisp = -(*fiatoshisp);
        *addrtypep = pubkey33[12];
        memcpy(rmd160,&pubkey33[13],20);
    }
    return(33);
}

uint64_t PAX_fiatdest(int32_t tokomodo,char *destaddr,uint8_t pubkey33[33],char *coinaddr,int32_t height,char *origbase,int64_t fiatoshis)
{
    uint8_t shortflag=0; char base[4]; int32_t i; uint8_t addrtype,rmd160[20]; int64_t komodoshis=0;
    for (i=0; i<3; i++)
        base[i] = toupper(origbase[i]);
    base[i] = 0;
    if ( strcmp(base,"KMD") == 0 )
        return(0);
    if ( fiatoshis < 0 )
        shortflag = 1, fiatoshis = -fiatoshis;
    komodoshis = dpow_paxprice(height,base,(char *)"KMD",(uint64_t)fiatoshis);
    if ( bitcoin_addr2rmd160(&addrtype,rmd160,coinaddr) == 20 )
    {
        PAX_pubkey(1,pubkey33,&addrtype,rmd160,base,&shortflag,tokomodo != 0 ? &komodoshis : &fiatoshis);
        bitcoin_address(destaddr,KOMODO_PUBTYPE,pubkey33,33);
    }
    return(komodoshis);
}

int32_t dpow_scriptitemlen(int32_t *opretlenp,uint8_t *script)
{
    int32_t opretlen,len = 0;
    if ( (opretlen= script[len++]) >= 0x4c )
    {
        if ( opretlen == 0x4c )
            opretlen = script[len++];
        else if ( opretlen == 0x4d )
        {
            opretlen = script[len++];
            opretlen = (opretlen << 8) | script[len++];
        }
    }
    *opretlenp = opretlen;
    return(len);
}

struct pax_transaction
{
    UT_hash_handle hh;
    bits256 txid;
    uint64_t komodoshis,fiatoshis;
    int32_t marked,height;
    uint16_t vout;
    char symbol[16],coinaddr[64]; uint8_t rmd160[20],shortflag;
} *PAX;

uint64_t dpow_paxtotal()
{
    struct pax_transaction *pax,*tmp; uint64_t total = 0;
    HASH_ITER(hh,PAX,pax,tmp)
    {
        if ( pax->marked == 0 )
            total += pax->fiatoshis;
    }
    return(total);
}

struct pax_transaction *dpow_paxfind(struct dpow_info *dp,struct pax_transaction *space,bits256 txid,uint16_t vout)
{
    struct pax_transaction *pax;
    pthread_mutex_lock(&dp->mutex);
    HASH_FIND(hh,PAX,&txid,sizeof(txid),pax);
    if ( pax != 0 )
        memcpy(space,pax,sizeof(*pax));
    pthread_mutex_unlock(&dp->mutex);
    return(pax);
}

struct pax_transaction *dpow_paxmark(struct dpow_info *dp,struct pax_transaction *space,bits256 txid,uint16_t vout,int32_t mark)
{
    struct pax_transaction *pax;
    pthread_mutex_lock(&dp->mutex);
    HASH_FIND(hh,PAX,&txid,sizeof(txid),pax);
    if ( pax == 0 )
    {
        pax = (struct pax_transaction *)calloc(1,sizeof(*pax));
        pax->txid = txid;
        pax->vout = vout;
        HASH_ADD_KEYPTR(hh,PAX,&pax->txid,sizeof(pax->txid),pax);
    }
    if ( pax != 0 )
    {
        pax->marked = mark;
        int32_t i; for (i=0; i<32; i++)
            printf("%02x",((uint8_t *)&txid)[i]);
        printf(" paxmark.ht %d vout%d\n",mark,vout);
        memcpy(space,pax,sizeof(*pax));
    }
    pthread_mutex_unlock(&dp->mutex);
    return(pax);
}

void dpow_issuer_withdraw(struct dpow_info *dp,char *coinaddr,uint64_t value,int32_t shortflag,char *symbol,uint64_t komodoshis,uint8_t *rmd160,bits256 txid,uint16_t vout,int32_t height) // assetchain context
{
    struct pax_transaction *pax;
    pthread_mutex_lock(&dp->mutex);
    HASH_FIND(hh,PAX,&txid,sizeof(txid),pax);
    if ( pax == 0 )
        pax = (struct pax_transaction *)calloc(1,sizeof(*pax));
    if ( coinaddr != 0 )
    {
        strcpy(pax->coinaddr,coinaddr);
        pax->komodoshis = komodoshis;
        pax->shortflag = shortflag;
        strcpy(pax->symbol,symbol);
        pax->fiatoshis = value;
        memcpy(pax->rmd160,rmd160,20);
        pax->height = height;
        if ( pax->marked == 0 )
            printf("ADD %.8f WITHDRAW %s %.8f -> %s TO PAX ht.%d\n",dstr(value),symbol,dstr(pax->fiatoshis),coinaddr,height);
        else printf("%.8f MARKED.%d WITHDRAW %s %.8f -> %s TO PAX ht.%d\n",dstr(value),pax->marked,symbol,dstr(pax->fiatoshis),coinaddr,height);
    }
    else
    {
        pax->marked = height;
        printf("MARK WITHDRAW ht.%d\n",height);
    }
    pax->txid = txid;
    pax->vout = vout;
    HASH_ADD_KEYPTR(hh,PAX,&pax->txid,sizeof(pax->txid),pax);
    pthread_mutex_unlock(&dp->mutex);
}

void dpow_issuer_voutupdate(struct dpow_info *dp,char *symbol,int32_t isspecial,int32_t height,int32_t txi,bits256 txid,int32_t vout,int32_t numvouts,uint64_t value,uint8_t *script,int32_t len)
{
    char base[16],destaddr[64],coinaddr[64]; uint8_t addrtype,shortflag,rmd160[20],pubkey33[33]; int64_t checktoshis,fiatoshis; struct pax_transaction space; int32_t i,kmdheight,opretlen,offset = 0;
    if ( script[offset++] == 0x6a )
    {
        offset += dpow_scriptitemlen(&opretlen,&script[offset]);
        if ( script[offset] == 'W' && strcmp(dp->symbol,"KMD") != 0 )
        {
            // if valid add to pricefeed for issue
            printf("WITHDRAW ht.%d txi.%d vout.%d %.8f opretlen.%d\n",height,txi,vout,dstr(value),opretlen);
            if ( opretlen == 38 ) // any KMD tx
            {
                memset(base,0,sizeof(base));
                offset++;
                offset += PAX_pubkey(0,&script[offset],&addrtype,rmd160,base,&shortflag,&fiatoshis);
                iguana_rwnum(0,&script[offset],sizeof(kmdheight),&kmdheight);
                if ( fiatoshis < 0 )
                    fiatoshis = -fiatoshis;
                bitcoin_address(coinaddr,addrtype,rmd160,20);
                checktoshis = PAX_fiatdest(1,destaddr,pubkey33,coinaddr,kmdheight,base,fiatoshis);
                if ( shortflag == dp->SHORTFLAG )
                {
                    if ( shortflag == 0 )
                    {
                        for (i=0; i<32; i++)
                            printf("%02x",((uint8_t *)&txid)[i]);
                        printf(" <- txid.v%u ",vout);
                        for (i=0; i<33; i++)
                            printf("%02x",pubkey33[i]);
                        printf(" checkpubkey check %.8f v %.8f dest.(%s) height.%d\n",dstr(checktoshis),dstr(value),destaddr,height);
                        if ( value <= fiatoshis )
                        {
                            if ( dpow_paxfind(dp,&space,txid,vout) == 0 )
                                dpow_issuer_withdraw(dp,coinaddr,fiatoshis,shortflag,base,checktoshis,rmd160,txid,vout,kmdheight);
                        }
                    }
                    else // short
                    {
                        for (i=0; i<opretlen; i++)
                            printf("%02x",script[i]);
                        printf(" opret[%c] value %.8f vs check %.8f\n",script[0],dstr(value),dstr(checktoshis));
                        if ( value >= fiatoshis )
                        {
                            
                        }
                    }
                }
            }
        }
        else if ( script[offset] == 'X' )
        {
            // scan from height in 'W'
            printf("WITHDRAW issued ht.%d txi.%d vout.%d %.8f\n",height,txi,vout,dstr(value));
        }
    }
}

int32_t dpow_issuer_tx(struct dpow_info *dp,struct iguana_info *coin,int32_t height,int32_t txi,char *txidstr,uint32_t port)
{
    char *retstr,params[256],*hexstr; uint8_t script[10000]; cJSON *json,*result,*vouts,*item,*sobj; int32_t vout,n,len,isspecial,retval = -1; uint64_t value; bits256 txid;
    sprintf(params,"[\"%s\", 1]",txidstr);
    if ( (retstr= dpow_issuemethod(coin->chain->userpass,(char *)"getrawtransaction",params,port)) != 0 )
    {
        if ( (json= cJSON_Parse(retstr)) != 0 )
        {
            //printf("TX.(%s)\n",retstr);
            if ( (result= jobj(json,(char *)"result")) != 0 && (vouts= jarray(&n,result,(char *)"vout")) != 0 )
            {
                retval = 0;
                isspecial = 0;
                txid = jbits256(result,(char *)"txid");
                for (vout=0; vout<n; vout++)
                {
                    item = jitem(vouts,vout);
                    value = SATOSHIDEN * jdouble(item,(char *)"value");
                    if ( (sobj= jobj(item,(char *)"scriptPubKey")) != 0 )
                    {
                        if ( (hexstr= jstr(sobj,(char *)"hex")) != 0 )
                        {
                            len = (int32_t)strlen(hexstr) >> 1;
                            if ( vout == 0 && ((memcmp(&hexstr[2],CRYPTO777_PUBSECPSTR,66) == 0 && len == 35) || (memcmp(&hexstr[6],CRYPTO777_RMD160STR,40) == 0 && len == 25)) )
                                isspecial = 1;
                            else if ( len <= sizeof(script) )
                            {
                                decode_hex(script,len,hexstr);
                                dpow_issuer_voutupdate(dp,coin->symbol,isspecial,height,txi,txid,vout,n,value,script,len);
                            }
                        }
                    }
                }
            } else printf("error getting txids.(%s)\n",retstr);
            free_json(json);
        }
        free(retstr);
    }
    return(retval);
}

int32_t dpow_issuer_block(struct dpow_info *dp,struct iguana_info *coin,int32_t height,uint16_t port)
{
    char *retstr,*retstr2,params[128],*txidstr; int32_t i,n,retval = -1; cJSON *json,*tx=0,*result=0,*result2;
    sprintf(params,"[%d]",height);
    if ( (retstr= dpow_issuemethod(coin->chain->userpass,(char *)"getblockhash",params,port)) != 0 )
    {
        if ( (result= cJSON_Parse(retstr)) != 0 )
        {
            if ( (txidstr= jstr(result,(char *)"result")) != 0 && strlen(txidstr) == 64 )
            {
                sprintf(params,"[\"%s\"]",txidstr);
                if ( (retstr2= dpow_issuemethod(coin->chain->userpass,(char *)"getblock",params,port)) != 0 )
                {
                    //printf("getblock.(%s)\n",retstr2);
                    if ( (json= cJSON_Parse(retstr2)) != 0 )
                    {
                        if ( (result2= jobj(json,(char *)"result")) != 0 && (tx= jarray(&n,result2,(char *)"tx")) != 0 )
                        {
                            for (i=0; i<n; i++)
                                if ( dpow_issuer_tx(dp,coin,height,i,jstri(tx,i),port) < 0 )
                                    break;
                            if ( i == n )
                                retval = 0;
                            else printf("dpow_issuer_block ht.%d error i.%d vs n.%d\n",height,i,n);
                        } else printf("cant get result.%p or tx.%p\n",result,tx);
                        free_json(json);
                    } else printf("cant parse2.(%s)\n",retstr2);
                    free(retstr2);
                } else printf("error getblock %s\n",params);
            } else printf("strlen.%ld (%s)\n",strlen(txidstr),txidstr);
            free_json(result);
        } else printf("couldnt parse.(%s)\n",retstr);
        free(retstr);
    } else printf("error from getblockhash %d\n",height);
    return(retval);
}

int32_t dpow_issuer_iteration(struct dpow_info *dp,struct iguana_info *coin,int32_t height,uint32_t *isrealtimep)
{
    char *retstr; int32_t i,kmdheight; cJSON *infoobj,*result; uint16_t port = coin->chain->rpcport;
    if ( height <= 0 )
        height = 1;
    *isrealtimep = 0;
    if ( (retstr= dpow_issuemethod(coin->chain->userpass,(char *)"getinfo",0,port)) != 0 )
    {
        if ( (infoobj= cJSON_Parse(retstr)) != 0 )
        {
            if ( (result= jobj(infoobj,(char *)"result")) != 0 && (kmdheight= jint(result,(char *)"blocks")) != 0 )
            {
                for (i=0; i<1000 && height<=kmdheight; i++,height++)
                {
                    /*fprintf(stderr,"%s.%d ",coin->symbol,height);
                    if ( (height % 10) == 0 )
                    {
                        if ( (height % 100) == 0 )
                            fprintf(stderr,"%s.%d ",coin->symbol,height);
                        memset(&zero,0,sizeof(zero));
                        komodo_stateupdate(height,0,0,0,zero,0,0,0,0,height,0,0,0,0);
                    }*/
                    if ( dpow_issuer_block(dp,coin,height,port) < 0 )
                    {
                        printf("error height %d\n",height);
                        break;
                    }
                    usleep(10000);
                }
                if ( height >= kmdheight )
                    *isrealtimep = (uint32_t)time(NULL);
            }
            free_json(infoobj);
        }
        //printf("GETINFO.(%s)\n",retstr);
        free(retstr);
    }
    else
    {
        printf("error from %s\n",coin->symbol);
        sleep(3);
    }
    //KOMODO_DEPOSIT = komodo_paxtotal();
    return(height);
}

