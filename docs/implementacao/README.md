# Implementação

Como o Zeebx é feito por dentro. A série `0x` na raiz de [`docs/`](../) é **pesquisa** — o que se
sabe do console e da plataforma; esta série é **implementação** — o que está escrito no código e
por quê.

A regra que vale em todos estes documentos: eles explicam **decisões**, não repetem o código.
Onde um número parece arbitrário, o texto diz de onde ele veio; onde uma escolha tinha
alternativa, o texto diz qual foi descartada e por quê. O que é óbvio lendo a assinatura da
função não está aqui.

| Doc | Conteúdo |
|---|---|
| [01-arquitetura.md](01-arquitetura.md) | Os módulos, o laço principal e como uma chamada do jogo vira trabalho no host |
| [02-cpu-e-memoria.md](02-cpu-e-memoria.md) | O núcleo ARM, o mapa de memória do guest, o heap e os objetos |
| [03-despacho-de-api.md](03-despacho-de-api.md) | Vtables, o trampolim, as interfaces e como um método novo entra |
| [04-tempo.md](04-tempo.md) | Relógio virtual, temporizadores, callbacks, threads e a espera ocupada |
| [05-video-2d.md](05-video-2d.md) | `IDisplay`, `IBitmap`, o DIB, o recorte e o custo da superfície |
| [06-video-3d.md](06-video-3d.md) | EGL, OpenGL ES, o rasterizador de software e os formatos de textura |
| [07-audio.md](07-audio.md) | RIFF/WAVE, IMA ADPCM, o misturador e `IMedia`/`ISound` |
| [08-arquivos-e-recursos.md](08-arquivos-e-recursos.md) | `.mod`, `.mif`, `.bar`, o sistema de arquivos virtual e os `.zip` |
| [09-entrada.md](09-entrada.md) | O controle do console, o `IHID` e o mapeamento configurável |
| [10-interface.md](10-interface.md) | Biblioteca, configurações, idiomas e o mapa visual do controle |
| [11-compatibilidade.md](11-compatibilidade.md) | O que roda, o que não roda e o que falta para cada caso |
| [12-rasterizador-e-paralelismo.md](12-rasterizador-e-paralelismo.md) | Por que dividir o trabalho por draw call não servia, e o que serviu |
| [13-classes-desconhecidas.md](13-classes-desconhecidas.md) | Como descobrir que interface é uma classe sem header: a sonda |
| [14-z-wheel-e-o-efs2.md](14-z-wheel-e-o-efs2.md) | Onde a loja do console parou, e o elo que falta no leitor da NAND |
| [15-o-que-falta-da-nand.md](15-o-que-falta-da-nand.md) | Classe a classe, o que ainda precisa sair do dump |
| [16-rede-e-a-ponte.md](16-rede-e-a-ponte.md) | HTTP, AES, e a ponte por módulo que o Zeeboids exigiu |

## Como medir

Duas ferramentas ficam no repositório justamente para não precisar adivinhar:

```bash
# vazão do núcleo ARM e custo de entrar no guest
cargo test --release cpu::unicorn::speed -- --ignored --nocapture

# um jogo sem janela, com o resumo de chamadas de API no fim
cargo run --release -- run caminho/para/jogo.mod --seconds=6 --trace
```

O `--trace` aceita filtro (`--trace=Bitmap`), e o resumo do fim conta **quantas vezes cada método
foi chamado** — foi ele que apontou os dois gargalos do Pac-Mania e as APIs que faltavam em nove
jogos.
