# Com um frontend seu

Quem já tem um frontend — um que simula a carcaça do console, uma estante de jogos, um gabinete
de fliperama — não quer a interface do Zeebx por cima da tela que ele mesmo montou. Para isso há
um binário sem interface nenhuma, configurado por um `config.ini`:

```bash
cargo build --release -p zeebx-headless
./target/release/zeebx-headless "roms/Quake.zip"
```

O jogo é obrigatório e não há padrão: este binário é chamado por outro programa, que sabe o que
quer abrir. Na primeira execução ele escreve um `config.ini` completo e comentado, e diz onde.
As opções e as chaves do arquivo são em inglês, como os comandos; os comentários são em
português.

Ele abre uma janela só com o jogo — ou nenhuma, mandando os quadros por um cano para o seu
programa pintar. Gráficos, áudio e controles saem dos mesmos campos que a interface grava, só
que em INI. Ver [`frontends/headless/LEIAME.md`](../frontends/headless/LEIAME.md).

O binário pronto sai em cada release como `zeebx-headless-<sistema>.zip`, com o `config.ini` e o
leia-me (ver [INSTALACAO.md](INSTALACAO.md#qual-arquivo-baixar)).