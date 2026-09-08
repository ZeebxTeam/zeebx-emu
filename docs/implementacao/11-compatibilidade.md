# 11 — Compatibilidade

Levantada rodando **as 61 ROMs** por seis segundos virtuais cada, sem janela. O procedimento está
no fim deste documento e dá para repetir a qualquer momento.

> "Roda" quer dizer **não quebrou em seis segundos**. Não quer dizer que a tela esteja certa nem
> que o jogo seja jogável. É o piso, não o teto.

## Placar

| Estado | Antes | Agora |
|---|---:|---:|
| roda | 33 | **45** |
| falha no `EVT_APP_START` | 10 | **0** |
| não cria o applet | 8 | 7 |
| para no laço de quadros | 7 | 6 |
| lento demais | 3 | 3 |

Doze jogos mudaram de estado de uma vez, e a causa foi uma só: **o sistema de arquivos do console
não distingue maiúsculas de minúsculas, e o nosso distinguia.** Os dez ports de arcade pedem
`font.fnz` e trazem `font.FNZ` no pacote; nenhum deles passava do `EVT_APP_START`, porque a fonte
não abria e o ponteiro nulo vinha logo depois. Raging Thunder 2 e Reckless Racing caíam no mesmo
buraco em outros arquivos. No Windows e no macOS isso funcionava por acaso.

Vale registrar como o defeito foi encontrado, porque o caminho não era óbvio: os dez pediam seis
ClassIDs que não temos, o que parecia ser a causa. **Não era** — eles seguem sem essas classes e
rodam assim mesmo. Quem entregou o problema foi o `DBGPRINTF` do próprio jogo, com um
`Failed to open font.fnz file!!!` na última linha do log, e a lista de "arquivos não encontrados"
do relatório logo abaixo confirmando.

## Por jogo

### Roda limpo (8)

Passou os seis segundos e o relatório não tem nada a apontar: nenhuma classe desconhecida, nenhuma
API atendida por hipótese, nenhum ponteiro recusado, nenhum arquivo não encontrado, nenhum texto
que não saibamos desenhar.

- Alien Breaker Deluxe
- Alpine Racer
- Treino Cerebral
- Zeebo F.C. Super League
- Zeebo Sports Peteca
- Zeebo Sports Queimada
- Zeebo Sports Tenis
- Zeebo Sports Volei

### Roda com ressalvas (37)

Passou os seis segundos, mas o relatório apontou alguma coisa. O balde é conservador: o
`config.cfg` que o Quake não acha é normal, porque só existe depois de salvar.

| Jogo | Classes que pede e não temos | Outras ressalvas |
|---|---|---|
| Armageddon Squadron |  | arquivo não encontrado, ponteiro recusado |
| Bad Dudes vs. DragonNinja | as seis do grupo de extensões | arquivo não encontrado, ponteiro recusado |
| Caveman Ninja | as seis do grupo de extensões | arquivo não encontrado, ponteiro recusado |
| Crash Bandicoot Nitro Kart 3D |  | arquivo não encontrado |
| Dark Seal | as seis do grupo de extensões | ponteiro recusado |
| Disney All Star Cards |  | API por hipótese, ponteiro recusado |
| Double Dragon | `0x0102f679` `0x0102f681` `0x01030852` | ponteiro recusado, texto sem fonte |
| FIFA 09 | `0x01001029` | arquivo não encontrado |
| Galaxy on Fire | as seis do grupo de extensões | arquivo não encontrado, ponteiro recusado |
| Heavy Barrel | as seis do grupo de extensões | arquivo não encontrado, ponteiro recusado |
| Iron Sight |  | arquivo não encontrado, ponteiro recusado |
| Karnovs Revenge | as seis do grupo de extensões | arquivo não encontrado, ponteiro recusado |
| Magical Drop 3 | as seis do grupo de extensões | arquivo não encontrado, ponteiro recusado |
| Pac-Mania |  | API por hipótese |
| Powerboat Challenge | `0x01001039` + as seis do grupo de extensões | arquivo não encontrado, ponteiro recusado |
| Quake | as seis do grupo de extensões | arquivo não encontrado |
| Quake 2 | as seis do grupo de extensões | arquivo não encontrado |
| Raging Thunder 2 |  | arquivo não encontrado, ponteiro recusado |
| Rally Master Pro | as seis do grupo de extensões | arquivo não encontrado, ponteiro recusado |
| Reckless Racing |  | arquivo não encontrado, ponteiro recusado |
| Resident Evil 4 - Zeebo Edition | `0x0102f681` | texto sem fonte |
| Ridge Racer | as seis do grupo de extensões |  |
| Spin Master | as seis do grupo de extensões | arquivo não encontrado, ponteiro recusado |
| Street Hoop | as seis do grupo de extensões | arquivo não encontrado, ponteiro recusado |
| Super BurgerTime | as seis do grupo de extensões | arquivo não encontrado, ponteiro recusado |
| Tekken 2 |  | API por hipótese |
| Ultimate Chess 3D | `0x01002000` `0x01005503` | arquivo não encontrado, ponteiro recusado |
| Um Jogo de Ovos | as seis do grupo de extensões | arquivo não encontrado |
| Wizard Fire | as seis do grupo de extensões | arquivo não encontrado, ponteiro recusado |
| Zeebo Clube | `0x0100110a` |  |
| Zeebo Extreme Baja |  | arquivo não encontrado |
| Zeebo Extreme Boia Cross |  | arquivo não encontrado |
| Zeebo Extreme Corrida Aerea |  | arquivo não encontrado |
| Zeebo Extreme Jetboard |  | arquivo não encontrado |
| Zeebo Extreme Rolima |  | arquivo não encontrado |
| Zeebo F.C. Foot Camp |  | arquivo não encontrado |
| Zeebo Family Pack |  | arquivo não encontrado, ponteiro recusado |

### Não roda (16)

| Jogo | Onde para |
|---|---|
| Action Hero 3D - Wild Dog and IMICRO3D | não chega a criar o applet |
| Alice no Pais das Maravilhas | lento demais — abaixo de 7% da velocidade |
| Bejeweled Twist | para no laço de quadros — acesso inválido a 0x00000024 (pc 0x00032b78) |
| Heavy Weapon | lento demais — abaixo de 7% da velocidade |
| Need For Speed - Carbon - Domine a Cidade | não chega a criar o applet |
| Peggle | para no laço de quadros — acesso inválido a 0x00000000 (pc 0x00019ab8) |
| Prey Evil | para no laço de quadros — acesso inválido a 0x00000000 (pc 0x00000000) |
| Tork and Kral - A Prehistorik Adventure | não chega a criar o applet |
| Toy Raid | para no laço de quadros — acesso inválido a 0x00000000 (pc 0x00018d4c) |
| Turma da Monica em Vamos Brincar Vol. 1 | lento demais — abaixo de 7% da velocidade |
| Z-Wheel | não chega a criar o applet |
| Zeebo App | não chega a criar o applet |
| Zeebo Channels - Opera Mini | não chega a criar o applet |
| Zeeboids | para no laço de quadros — API não implementada — IHash::slot[4] |
| Zenonia | não chega a criar o applet |
| Zumas Revenge | para no laço de quadros — exceção do núcleo ARM em pc 0x00010f94 |

### As seis classes do grupo de extensões

```
0x0103d8de  0x0103d8eb  0x0103d8ef  0x0103d8f0  0x0103def1  0x010426e3
```

Pedidas por dezessete jogos, e **nenhum deles precisa delas para rodar**: todos recebem
`ECLASSNOTSUPPORT` e caem no caminho alternativo. O log do Quake e o dos ports de arcade mostram
de onde vêm — o `GLES_ext.c` do próprio SDK do BREW, que os jogos linkam:

```
eglGetProcAddress (NBI) - got V2 EGLSurfaceManip interface
eglGetProcAddress (NBI) - got V2 GLESImageonExt interface
```

Ou seja: são as extensões de OpenGL ES do console, e as duas que os jogos anunciam ter conseguido
são justamente as que já implementamos. O que essas seis fazem, e se alguma muda o desenho na
tela, ainda não foi levantado — mas nenhuma delas impede jogo nenhum de rodar hoje.

## A largura do `printf` era um defeito invisível

Vale registrar porque não aparece como quebra nenhuma nesta tabela.

O `cformat` lia as flags e a largura do especificador e **descartava** — o que serve para o log do
`DBGPRINTF`, que era o uso original, mas o mesmo formatador atende o `snprintf` e o `vsnprintf`
do guest. O Resident Evil 4 monta o nome dos arquivos de estágio com `%s_%02d.h2z`; sem a
largura, o `%02d` de zero saía como `0`, ele procurava `3d_stg02_0.h2z` e o arquivo é
`3d_stg02_00.h2z`. **Os doze estágios não eram encontrados**, e o jogo rodava sem textura
nenhuma — sem quebrar, sem erro, sem nada no relatório além de uma lista de "arquivos não
encontrados" que ninguém tinha lido.

Foi a mesma lista que entregou o `font.fnz` dos ports de arcade, e nas duas vezes ela estava lá
desde o começo. A lição: **a lista de arquivos não encontrados do relatório é sinal, não ruído.**

## Como repetir

```bash
for z in roms/*.zip; do
  n=$(basename "$z" .zip)
  timeout 90 ./target/release/zeebx run "$z" --seconds=6 > "saida/$n.txt" 2>&1
done
```

O emulador extrai o `.zip` sozinho e escolhe o `.mod` certo, então não é mais preciso desempacotar
à mão. Quem estourar o `timeout` é "lento demais".

O que ler em cada saída, **nesta ordem** — é ela que separa as categorias:

| Linha | Significa |
|---|---|
| `applet: nenhum .mif encontrado` | nem chegou a criar o applet |
| `start: EVT_APP_START → retornou …` | o applet foi criado e recebeu o evento inicial |
| `start: EVT_APP_START → acesso inválido …` | quebrou no evento inicial, antes do primeiro quadro |
| `parou na volta N em …` | quebrou já dentro do laço de quadros, na volta N |
| `arquivos não encontrados:` | **leia sempre** — um nome errado aqui vira tela preta ou quebra, sem erro nenhum |
| `classes desconhecidas: …` | ClassIDs que o jogo pediu e não temos |
| `chamadas:` | quantas vezes cada método foi chamado — é o que aponta gargalo |
| `log do jogo:` | os `DBGPRINTF` do próprio jogo, que costumam nomear o problema |
