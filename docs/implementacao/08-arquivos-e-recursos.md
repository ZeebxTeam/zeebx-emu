# 08 — Arquivos e recursos

## Os formatos

| Formato | Módulo | O que é |
|---|---|---|
| `.mod` | `modfile.rs` | o executável ARM do módulo |
| `.mif` | `miffile.rs` | o Module Information File: ClassIDs dos applets e os ícones |
| `.bar` | `resfile.rs` | recursos — strings e imagens que o jogo carrega por id |
| `.zip` | `archive.rs` | como os títulos costumam circular |

### `.mif`

O formato não é público. O que está no código foi levantado comparando **24 arquivos reais**: os
do SDK (cujos ClassIDs conhecemos pelos `.bid`), os módulos de sistema do simulador, os homebrew
do OpenZeebo e os títulos do console.

O índice de seções tem `n + 1` offsets — cada seção vai de `offs[i]` a `offs[i+1]`, e o último
valor é o tamanho do arquivo. O registro de applet tem **20 bytes** e começa com o `AEECLSID`; em
todos os `.mif` de applet examinados existe exatamente uma seção desse tamanho.

A regra foi validada contra valores conhecidos por outra fonte: `mediaplayer.mif` devolve
`0x01010EF6`, o mesmo ClassID que a engenharia reversa da firmware registrou para o Media Player.

**Os ícones** ficam em seções que começam com um `u16` de comprimento **do próprio cabeçalho**
(sempre 12) seguido do tipo MIME terminado em zero — `image/png`, `image/bmp` ou `image/jpg`. Ler
esse comprimento como se fosse só o do texto deixa dois bytes para trás e nenhum decodificador
aceita a imagem.

Tamanhos reais: 16×16, 26×26 e 65×42 na maioria, 44×44 no Crash, **192×192** no Resident Evil 4.
**Não existe arte de capa dentro das ROMs.**

### `.bar`

Mesmo contêiner do `.mif`. As entradas do índice são `(u16 tipo, u16 id, u16 a, u16 b)`, onde os
ids `id..=id+a` vivem nas seções `b..=b+a`. Tipos: `1` string, `6` imagem (com cabeçalho
`AEEResBlob`), `0x5000` binário.

## O sistema de arquivos virtual

`vfs.rs`. O BREW dá a cada módulo um diretório próprio, e é para lá que os caminhos do jogo
apontam:

```
arquivo.dat          relativo ao diretório do módulo
fs:/~/arquivo.dat    o "~" é o diretório do próprio módulo
fs:/~/../id1/x       sobe para a raiz de módulos — o Quake guarda os dados dele assim
fs:/~0x01234567/x    diretório de outro módulo, pelo ClassID
fs:/shared/x         área compartilhada
```

Qualquer caminho que escape da raiz é recusado — `..` além do limite, caminho absoluto do host,
raiz do Windows. **Um `.mod` de origem desconhecida não deveria conseguir ler o resto da
máquina.** O limite superior é a raiz de módulos, porque o Quake legitimamente sobe até lá.

`resolve` recusa o próprio diretório do módulo; `resolve_dir` o aceita. A diferença é exatamente
essa: listar `fs:/~/` é legítimo, abri-lo como arquivo não.

## Enumeração de diretório

`IFileMgr::EnumInit`/`EnumNext` destravaram cinco jogos. A listagem inteira sai de uma vez no
`EnumInit`, e a fila fica **dentro do `IFileMgr`** — como no BREW, de modo que dois gerenciadores
enumeram diretórios diferentes ao mesmo tempo.

O nome devolvido é o **caminho completo**, com o mesmo prefixo que o jogo passou: é ele que volta
para o `OpenFile` logo em seguida, e um nome solto não abriria nada.

Depois de um `EnumNext` falso, o `GetLastError` devolve `EFAILED` mesmo tendo dado tudo certo. A
documentação chama isso de compatibilidade com o cliente 1.0, e há jogo que confere.

## `.zip`

O emulador **extrai para um cache** antes de rodar, em vez de ler de dentro do pacote. Os jogos
gravam — o Peteca tem um `.sav` —, e escrever de volta num zip não é coisa que se queira fazer.
Extrair resolve isso de graça e deixa o resto do emulador sem saber que o zip existe.

A pasta do cache é nomeada pelo arquivo mais o tamanho e a data, de modo que trocar a ROM invalida
a extração antiga.

Havendo mais de um `.mod` dentro, ganha o que está na disposição do console —
`<Título>/mod/<id>/<nome>.mod` — mesmo que seja o mais fundo: um pacote que traz o jogo e algum
extra tem o módulo do jogo justamente ali. Empatado isso, vale o caminho mais curto.

## `IUnzipAStream`

`AEECLSID_UNZIPSTREAM` descomprime um stream. O Double Dragon lê o `data.ggz` dele assim, a
partir de um `IFile` — e um `IFile` também é um `IAStream`. A descompressão tenta gzip, zlib e
deflate cru, nessa ordem, porque o que chega não anuncia qual é.

## Imagens dos jogos

`icon.rs` decodifica PNG (expandindo paleta), **BMP paletado de 1/4/8 bits e direto de 24/32** e
JPEG. O BMP é escrito à mão porque o que os `.mif` trazem é o subconjunto mais simples do formato,
e uma dependência inteira custaria mais que as poucas dezenas de linhas.

## O decodificador de PNG do BREW

`AEECLSID_PNGDecoderBREW` (`0x01030766`) é uma classe à parte do `AEECLSID_PNG`: a interface
padrão dela é `IImageDecoder`, não `IImage`. O caminho que os jogos usam:

```
ISHELL_CreateInstance(AEECLSID_PNGDecoderBREW)   -> IImageDecoder
IIMAGEDECODER_QueryInterface(AEEIID_FORCEFEED)   -> IForceFeed
IFORCEFEED_Write(pedaço, n) …                    -> o arquivo, aos poucos
IFORCEFEED_Write(NULL, 0)                        -> fim
IIMAGEDECODER_GetBitmap(&bmp)                    -> o IBitmap pronto
IIMAGEDECODER_GetRop()                           -> com o que desenhá-lo
```

O `IForceFeed` é um objeto **separado** do decodificador. As duas interfaces têm métodos
diferentes no mesmo slot — `GetBitmap` e `Write` são ambos o slot 3 —, e devolver o mesmo
ponteiro faria o jogo chamar o método errado.

A decodificação acontece no `GetBitmap`, não na escrita de fim: adiantá-la só gastaria trabalho
se o jogo desistisse no meio.

**`AEEIID_FORCEFEED = 0x0101eb0b` não está nos headers que temos.** Foi identificada pelo uso: o
`.bid` da classe declara `IForceFeed` entre as interfaces suportadas, é essa a interface que
alimenta um decodificador, e é essa IID que o Heavy Weapon pede logo depois de criar o objeto. O
comportamento confirma — ele passa a decodificar as imagens dele.

O formato sai da **assinatura do arquivo**, não do rótulo: um `.mif` chama o JPEG de `image/jpg`,
e seguir o rótulo à risca deixaria o ícone de fora por causa de uma letra.
