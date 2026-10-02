//! Os controles desenhados na tela de um celular: onde fica cada peça e o que o dedo aperta.
//!
//! Mora no núcleo, e não no frontend do Android, por duas razões. O pacote do Android só compila
//! para o Android, e a conta de qual botão o dedo acertou é a parte que vale testar; e o iOS vai
//! precisar da mesma conta. O desenho e a leitura do evento de toque ficam com cada frontend.
//!
//! Tudo aqui é em **pontos** da interface, com a origem no canto de cima à esquerda e o `y`
//! crescendo para baixo — o mesmo espaço do egui.

use std::collections::BTreeMap;

use super::DPAD;

/// Uma peça da sobreposição.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Peca {
    Direcional,
    /// O manche esquerdo, nos eixos `X` e `Y`.
    MancheEsquerdo,
    /// O manche direito, nos eixos `Z` e `RZ`.
    MancheDireito,
    B1,
    B2,
    B3,
    B4,
    Zl,
    Zr,
    Home,
}

impl Peca {
    pub const TODAS: [Self; 10] = [
        Self::Direcional,
        Self::MancheEsquerdo,
        Self::MancheDireito,
        Self::B1,
        Self::B2,
        Self::B3,
        Self::B4,
        Self::Zl,
        Self::Zr,
        Self::Home,
    ];

    /// A chave da peça nas posições e nos tamanhos gravados. Não muda nunca: é o que está no
    /// `settings.json`.
    ///
    /// **Os quatro botões de face têm uma chave só**, `botoes`: no editor eles são um grupo, que
    /// se arrasta e se redimensiona inteiro. Separados, arrumar o losango pedia quatro arrastos
    /// alinhados à mão, e ele nunca voltava a ser um losango.
    pub fn chave(self) -> &'static str {
        match self {
            Self::Direcional => "dpad",
            Self::MancheEsquerdo => "lstick",
            Self::MancheDireito => "rstick",
            Self::B1 | Self::B2 | Self::B3 | Self::B4 => "botoes",
            Self::Zl => "zl",
            Self::Zr => "zr",
            Self::Home => "home",
        }
    }

    /// Onde o botão fica no losango, em raios a partir do centro dele. A posição é a do
    /// controle: o 1 embaixo, o 2 à esquerda, o 3 em cima e o 4 à direita (issue #41).
    fn no_losango(self) -> Option<[f32; 2]> {
        match self {
            Self::B1 => Some([0.0, 1.0]),
            Self::B2 => Some([-1.0, 0.0]),
            Self::B3 => Some([0.0, -1.0]),
            Self::B4 => Some([1.0, 0.0]),
            _ => None,
        }
    }

    /// O que vai escrito na peça — o mesmo que está impresso no controle do Zeebo.
    pub fn rotulo(self) -> &'static str {
        match self {
            Self::Direcional | Self::MancheEsquerdo | Self::MancheDireito => "",
            Self::B1 => "1",
            Self::B2 => "2",
            Self::B3 => "3",
            Self::B4 => "4",
            Self::Zl => "ZL",
            Self::Zr => "ZR",
            Self::Home => "HOME",
        }
    }

    /// O botão do [`super::Pad`] que a peça aperta. O direcional aperta quatro, e por isso não
    /// tem um só; os manches não apertam botão nenhum.
    ///
    /// Os índices são os mesmos que o frontend do Android dá ao controle físico: o ZL e o ZR são
    /// os "superiores" de cada lado, e o HOME ocupa o `Back` — ver [`super::BUTTON_UIDS`].
    pub fn indice(self) -> Option<usize> {
        match self {
            Self::Direcional | Self::MancheEsquerdo | Self::MancheDireito => None,
            Self::B1 => Some(0),
            Self::B2 => Some(1),
            Self::B3 => Some(2),
            Self::B4 => Some(3),
            Self::Zl => Some(6),
            Self::Zr => Some(4),
            Self::Home => Some(9),
        }
    }

    /// Os dois eixos do [`super::Pad`] que o manche move, horizontal e vertical. Na ordem de
    /// [`super::Pad::axes`]: `X`, `Y`, `Z`, `RZ`.
    pub fn eixos(self) -> Option<[usize; 2]> {
        match self {
            Self::MancheEsquerdo => Some([0, 1]),
            Self::MancheDireito => Some([2, 3]),
            _ => None,
        }
    }

    /// Redonda ou pílula. Os gatilhos são pílulas, como no controle.
    pub fn redonda(self) -> bool {
        !matches!(self, Self::Zl | Self::Zr)
    }

    /// Metade da largura e da altura, em pontos, no tamanho de fábrica.
    ///
    /// Os botões de face têm 60 pontos de diâmetro, acima dos 48 que o Android pede para um alvo
    /// de toque: no jogo o polegar não olha para onde vai.
    fn meio(self) -> [f32; 2] {
        match self {
            Self::Direcional => [72.0, 72.0],
            Self::MancheEsquerdo | Self::MancheDireito => [50.0, 50.0],
            Self::B1 | Self::B2 | Self::B3 | Self::B4 => [30.0, 30.0],
            Self::Zl | Self::Zr => [44.0, 22.0],
            Self::Home => [30.0, 18.0],
        }
    }
}

/// Quanto da borda fica livre em volta das peças, no lugar de fábrica.
const MARGEM: f32 = 28.0;

/// A distância do centro do losango de botões até o centro de cada um.
const RAIO_DO_LOSANGO: f32 = 62.0;

/// O vão entre peças vizinhas no lugar de fábrica.
const VAO: f32 = 16.0;

/// Quanto além do desenho o toque ainda vale. O polegar escorrega durante o jogo, e um botão que
/// só obedece dentro do círculo pintado parece falhar.
const FOLGA: f32 = 1.15;

/// No direcional a folga é maior: é a peça que o dedo segura arrastando.
const FOLGA_DO_DIRECIONAL: f32 = 1.35;

/// Perto do centro do direcional nenhuma direção vale. Sem isto, o polegar parado em cima dele
/// trocaria de direção a cada tremida.
const CENTRO_MORTO: f32 = 0.2;

/// Abaixo disto, em fração do raio, o manche está no centro. É a mesma zona morta do manche
/// físico: o polegar parado em cima dele nunca está parado de verdade.
const ZONA_MORTA_DO_MANCHE: f32 = 0.12;

/// `sen(22,5°)`: com ele cada direção vale num setor de 135°, e as quatro juntas cortam o círculo
/// em oito fatias de 45° — quatro retas e quatro diagonais, como num direcional de verdade.
const LIMIAR_DA_DIAGONAL: f32 = 0.382_683_43;

/// Uma peça posta na tela.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Elemento {
    pub peca: Peca,
    pub centro: [f32; 2],
    /// Metade da largura e da altura, já na escala escolhida.
    pub meio: [f32; 2],
}

/// Onde cada peça fica de fábrica, em pontos, numa tela do tamanho dado.
///
/// O lugar de fábrica é dito **a partir das bordas**, e não em fração da tela: o losango dos
/// botões precisa continuar um losango num celular comprido e num tablet quase quadrado.
fn lugar_de_fabrica(peca: Peca, tela: [f32; 2], escala: f32) -> [f32; 2] {
    let [largura, altura] = tela;
    let meio = peca.meio().map(|v| v * escala);
    let losango = [
        largura - MARGEM - (RAIO_DO_LOSANGO + 30.0) * escala,
        altura - MARGEM - (RAIO_DO_LOSANGO + 30.0) * escala,
    ];
    let passo = RAIO_DO_LOSANGO * escala;
    let direcional = Peca::Direcional.meio()[0] * escala;
    let botao = Peca::B2.meio()[0] * escala;
    match peca {
        Peca::Direcional => [MARGEM + meio[0], altura - MARGEM - meio[1]],
        // Os manches embaixo, para dentro do direcional e do losango, como no controle: o
        // direcional e os botões ficam no alcance natural do polegar, e o manche um pouco abaixo
        // e para o centro.
        Peca::MancheEsquerdo => [
            MARGEM + 2.0 * direcional + VAO * escala + meio[0],
            altura - MARGEM - meio[1],
        ],
        Peca::MancheDireito => [
            losango[0] - passo - botao - VAO * escala - meio[0],
            altura - MARGEM - meio[1],
        ],
        // Dos botões, o lugar de fábrica é o do centro do losango; cada um sai dali em
        // [`monta`], que é quem sabe o tamanho do grupo.
        Peca::B1 | Peca::B2 | Peca::B3 | Peca::B4 => losango,
        Peca::Zl => [MARGEM + meio[0], MARGEM + meio[1]],
        Peca::Zr => [largura - MARGEM - meio[0], MARGEM + meio[1]],
        // Em cima, no meio: embaixo é onde os manches moram.
        Peca::Home => [largura / 2.0, MARGEM / 2.0 + meio[1]],
    }
}

/// O menor e o maior tamanho de uma peça no editor, em porcento. Abaixo da metade o botão fica
/// menor que o alvo de toque que o Android pede; acima do dobro, o direcional sozinho passa de um
/// terço da altura de um celular deitado.
pub const TAMANHO_MINIMO: u8 = 50;
pub const TAMANHO_MAXIMO: u8 = 200;

/// Põe as peças numa tela do tamanho dado.
///
/// `escala` é em porcento do tamanho de fábrica, e vale para todas. `tamanhos` é o de cada peça,
/// pela [`Peca::chave`], em porcento, por cima da `escala`; a peça que não está ali fica em 100.
/// `posicoes` é o que o usuário arrastou, em fração da tela; a peça que não está ali fica no
/// lugar de fábrica. Qualquer que seja a posição gravada, a peça termina inteira dentro da tela:
/// um arquivo feito num tablet e aberto num celular não pode deixar um botão para fora.
pub fn monta(
    tela: [f32; 2],
    escala: u8,
    posicoes: &BTreeMap<String, [f32; 2]>,
    tamanhos: &BTreeMap<String, u8>,
) -> Vec<Elemento> {
    let escala = f32::from(escala.max(1)) / 100.0;
    Peca::TODAS
        .iter()
        .map(|&peca| {
            // O lugar de fábrica é contado com a escala geral, e não com a da peça: crescer o
            // direcional não pode empurrar os manches de lugar.
            let propria = tamanhos
                .get(peca.chave())
                .map_or(100, |t| (*t).clamp(TAMANHO_MINIMO, TAMANHO_MAXIMO));
            let tamanho = escala * f32::from(propria) / 100.0;
            let meio = peca.meio().map(|v| v * tamanho);
            let centro = match posicoes.get(peca.chave()) {
                Some(&[fx, fy]) => [fx * tela[0], fy * tela[1]],
                None => lugar_de_fabrica(peca, tela, escala),
            };
            let centro = match peca.no_losango() {
                // Do botão, o centro gravado é o do losango, e quem tem de caber é o losango
                // inteiro: prender cada botão por si o deformaria contra a borda.
                Some([dx, dy]) => {
                    let passo = RAIO_DO_LOSANGO * tamanho;
                    let alcance = [passo + meio[0], passo + meio[1]];
                    [
                        prende(centro[0], alcance[0], tela[0]) + dx * passo,
                        prende(centro[1], alcance[1], tela[1]) + dy * passo,
                    ]
                }
                None => [
                    prende(centro[0], meio[0], tela[0]),
                    prende(centro[1], meio[1], tela[1]),
                ],
            };
            Elemento { peca, centro, meio }
        })
        .collect()
}

/// Prende uma coordenada para a peça caber; se a tela for menor que a peça, ela fica no meio.
fn prende(valor: f32, meio: f32, limite: f32) -> f32 {
    match limite > 2.0 * meio {
        true => valor.clamp(meio, limite - meio),
        false => limite / 2.0,
    }
}

/// O inverso de [`monta`] para uma peça: de um centro em pontos à fração que se grava.
pub fn fracao(centro: [f32; 2], tela: [f32; 2]) -> [f32; 2] {
    [
        (centro[0] / tela[0].max(1.0)).clamp(0.0, 1.0),
        (centro[1] / tela[1].max(1.0)).clamp(0.0, 1.0),
    ]
}

/// Os botões que os dedos apertam, um bit por índice do [`super::Pad`].
///
/// Não guarda estado entre chamadas: o Android manda, em todo evento de toque, a posição de
/// todos os dedos na tela, e o que está apertado é só função de onde eles estão. Assim o dedo que
/// escorrega de um botão para o vizinho troca o aperto sem precisar levantar.
pub fn botoes(elementos: &[Elemento], dedos: impl IntoIterator<Item = [f32; 2]>) -> u32 {
    let mut bits = 0;
    for dedo in dedos {
        for elemento in elementos {
            bits |= aperta(elemento, dedo);
        }
    }
    bits
}

/// Uma peça e um dedo: o que ele aperta nela.
fn aperta(elemento: &Elemento, dedo: [f32; 2]) -> u32 {
    let dx = dedo[0] - elemento.centro[0];
    let dy = dedo[1] - elemento.centro[1];
    let [mx, my] = elemento.meio;
    match elemento.peca {
        Peca::MancheEsquerdo | Peca::MancheDireito => 0,
        Peca::Direcional => {
            let distancia = dx.hypot(dy);
            if distancia > mx * FOLGA_DO_DIRECIONAL || distancia < mx * CENTRO_MORTO {
                return 0;
            }
            let limite = distancia * LIMIAR_DA_DIAGONAL;
            let [cima, baixo, esquerda, direita] = DPAD;
            [
                (dy < -limite, cima),
                (dy > limite, baixo),
                (dx < -limite, esquerda),
                (dx > limite, direita),
            ]
            .into_iter()
            .filter(|(vale, _)| *vale)
            .fold(0, |bits, (_, indice)| bits | 1 << indice)
        }
        peca => {
            let dentro = match peca.redonda() {
                // Uma elipse, que é o círculo quando os dois meios são iguais — o HOME é mais
                // largo que alto.
                true => (dx / (mx * FOLGA)).powi(2) + (dy / (my * FOLGA)).powi(2) <= 1.0,
                false => dx.abs() <= mx * FOLGA && dy.abs() <= my * FOLGA,
            };
            match (dentro, peca.indice()) {
                (true, Some(indice)) => 1 << indice,
                _ => 0,
            }
        }
    }
}

/// O manche em que um dedo que acabou de encostar pegou, se pegou em algum.
///
/// É só na descida que se pergunta: daí em diante o dedo é do manche até levantar, saia ele do
/// círculo ou não. Sem isso, empurrar com força até a borda faria o dedo sair da peça e o manche
/// voltar ao centro no meio da curva.
pub fn pega_manche(elementos: &[Elemento], dedo: [f32; 2]) -> Option<Peca> {
    elementos.iter().find_map(|elemento| {
        elemento.peca.eixos()?;
        let dx = dedo[0] - elemento.centro[0];
        let dy = dedo[1] - elemento.centro[1];
        (dx.hypot(dy) <= elemento.meio[0] * FOLGA).then_some(elemento.peca)
    })
}

/// Onde o dedo põe o manche: horizontal e vertical, de `-1` a `1`, com o `y` crescendo para
/// baixo — que é também o sentido do eixo interno do [`super::Pad`], onde cima é negativo.
///
/// Fora do círculo, o manche fica na borda, na direção do dedo.
pub fn manche(elemento: &Elemento, dedo: [f32; 2]) -> [f32; 2] {
    let raio = elemento.meio[0].max(1.0);
    let x = (dedo[0] - elemento.centro[0]) / raio;
    let y = (dedo[1] - elemento.centro[1]) / raio;
    let tamanho = x.hypot(y);
    if tamanho < ZONA_MORTA_DO_MANCHE {
        return [0.0, 0.0];
    }
    let corte = tamanho.max(1.0);
    [x / corte, y / corte]
}

/// O centro do que o editor arrasta quando pega a peça: o do losango, para um botão de face, e o
/// da própria peça para o resto. É o ponto que se grava em [`fracao`].
pub fn centro_do_grupo(elementos: &[Elemento], peca: Peca) -> Option<[f32; 2]> {
    let grupo: Vec<_> = elementos
        .iter()
        .filter(|e| e.peca.chave() == peca.chave())
        .collect();
    if grupo.is_empty() {
        return None;
    }
    let n = grupo.len() as f32;
    Some([
        grupo.iter().map(|e| e.centro[0]).sum::<f32>() / n,
        grupo.iter().map(|e| e.centro[1]).sum::<f32>() / n,
    ])
}

/// A peça que está debaixo de um ponto, para o editor saber qual o dedo pegou. Sem folga: no
/// editor ninguém está jogando, e a folga faria pegar a peça vizinha.
pub fn debaixo(elementos: &[Elemento], ponto: [f32; 2]) -> Option<Peca> {
    // De trás para a frente: quem foi desenhado por último está por cima.
    elementos.iter().rev().find_map(|elemento| {
        let dx = (ponto[0] - elemento.centro[0]).abs();
        let dy = (ponto[1] - elemento.centro[1]).abs();
        (dx <= elemento.meio[0] && dy <= elemento.meio[1]).then_some(elemento.peca)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Um celular comum deitado: 20:9, em pontos.
    const TELA: [f32; 2] = [915.0, 412.0];

    fn fabrica() -> Vec<Elemento> {
        monta(TELA, 100, &BTreeMap::new(), &BTreeMap::new())
    }

    fn centro(elementos: &[Elemento], peca: Peca) -> [f32; 2] {
        elementos.iter().find(|e| e.peca == peca).unwrap().centro
    }

    fn bit(indice: usize) -> u32 {
        1 << indice
    }

    #[test]
    fn cada_botao_aperta_o_seu_indice() {
        let elementos = fabrica();
        for peca in Peca::TODAS {
            let Some(indice) = peca.indice() else { continue };
            assert_eq!(
                botoes(&elementos, [centro(&elementos, peca)]),
                bit(indice),
                "{peca:?}"
            );
        }
    }

    #[test]
    fn as_pecas_de_fabrica_cabem_e_nao_se_tocam() {
        for tela in [TELA, [1024.0, 768.0], [640.0, 360.0]] {
            let elementos = monta(tela, 100, &BTreeMap::new(), &BTreeMap::new());
            for (i, a) in elementos.iter().enumerate() {
                assert!(a.centro[0] - a.meio[0] >= 0.0 && a.centro[0] + a.meio[0] <= tela[0]);
                assert!(a.centro[1] - a.meio[1] >= 0.0 && a.centro[1] + a.meio[1] <= tela[1]);
                for b in &elementos[i + 1..] {
                    let separadas = (a.centro[0] - b.centro[0]).abs() >= a.meio[0] + b.meio[0]
                        || (a.centro[1] - b.centro[1]).abs() >= a.meio[1] + b.meio[1];
                    assert!(separadas, "{:?} e {:?} em {tela:?}", a.peca, b.peca);
                }
            }
        }
    }

    #[test]
    fn o_direcional_tem_oito_direcoes_e_um_centro_morto() {
        let elementos = fabrica();
        let [x, y] = centro(&elementos, Peca::Direcional);
        let [cima, baixo, esquerda, direita] = DPAD;
        assert_eq!(botoes(&elementos, [[x, y]]), 0);
        assert_eq!(botoes(&elementos, [[x, y - 50.0]]), bit(cima));
        assert_eq!(botoes(&elementos, [[x - 50.0, y]]), bit(esquerda));
        assert_eq!(botoes(&elementos, [[x + 35.0, y + 35.0]]), bit(baixo) | bit(direita));
        // A 20° da horizontal ainda é reta: a diagonal só começa nos 22,5°.
        let (s, c) = 20f32.to_radians().sin_cos();
        assert_eq!(botoes(&elementos, [[x + 50.0 * c, y - 50.0 * s]]), bit(direita));
        // Fora do desenho, mas dentro da folga, ainda vale.
        assert_eq!(botoes(&elementos, [[x, y + 90.0]]), bit(baixo));
        assert_eq!(botoes(&elementos, [[x, y + 110.0]]), 0);
    }

    #[test]
    fn dois_dedos_somam() {
        let elementos = fabrica();
        let [x, y] = centro(&elementos, Peca::Direcional);
        let b1 = centro(&elementos, Peca::B1);
        assert_eq!(
            botoes(&elementos, [[x, y - 50.0], b1]),
            bit(DPAD[0]) | bit(0)
        );
    }

    #[test]
    fn o_manche_segue_o_dedo_e_para_na_borda() {
        let elementos = fabrica();
        let esquerdo = *elementos.iter().find(|e| e.peca == Peca::MancheEsquerdo).unwrap();
        let [x, y] = esquerdo.centro;
        assert_eq!(manche(&esquerdo, [x, y]), [0.0, 0.0]);
        assert_eq!(manche(&esquerdo, [x + 25.0, y]), [0.5, 0.0]);
        // Cima é negativo, como no eixo do console.
        assert_eq!(manche(&esquerdo, [x, y - 50.0]), [0.0, -1.0]);
        assert_eq!(manche(&esquerdo, [x, y + 300.0]), [0.0, 1.0]);
        let [dx, dy] = manche(&esquerdo, [x + 100.0, y - 100.0]);
        assert!((dx.hypot(dy) - 1.0).abs() < 1e-5 && dx > 0.0 && dy < 0.0);
        // Tremida no centro não move nada.
        assert_eq!(manche(&esquerdo, [x + 4.0, y]), [0.0, 0.0]);
    }

    #[test]
    fn so_o_manche_pega_o_dedo_e_ele_nao_aperta_botao() {
        let elementos = fabrica();
        let direito = centro(&elementos, Peca::MancheDireito);
        assert_eq!(pega_manche(&elementos, direito), Some(Peca::MancheDireito));
        assert_eq!(pega_manche(&elementos, centro(&elementos, Peca::B1)), None);
        assert_eq!(botoes(&elementos, [direito]), 0);
        assert_eq!(Peca::MancheDireito.eixos(), Some([2, 3]));
    }

    #[test]
    fn longe_de_tudo_nao_aperta_nada() {
        assert_eq!(botoes(&fabrica(), [[TELA[0] / 2.0, TELA[1] / 3.0]]), 0);
    }

    #[test]
    fn a_posicao_gravada_e_fracao_e_a_peca_fica_dentro() {
        let mut posicoes = BTreeMap::new();
        posicoes.insert("botoes".to_string(), [0.5, 0.5]);
        posicoes.insert("zl".to_string(), [0.0, 0.0]);
        let elementos = monta(TELA, 100, &posicoes, &BTreeMap::new());
        // O grupo inteiro foi para o meio: o 1 fica um raio abaixo do centro.
        assert_eq!(centro(&elementos, Peca::B1), [457.5, 268.0]);
        assert_eq!(centro_do_grupo(&elementos, Peca::B3), Some([457.5, 206.0]));
        // Gravado no canto, desenhado encostado nele.
        assert_eq!(centro(&elementos, Peca::Zl), [44.0, 22.0]);
        assert_eq!(fracao([457.5, 206.0], TELA), [0.5, 0.5]);
    }

    #[test]
    fn a_escala_aumenta_a_peca_e_o_alcance() {
        let normal = fabrica();
        let [x, y] = centro(&normal, Peca::B1);
        assert_eq!(botoes(&normal, [[x + 40.0, y]]), 0);
        let maior = monta(TELA, 150, &BTreeMap::new(), &BTreeMap::new());
        let [x, y] = centro(&maior, Peca::B1);
        assert_eq!(botoes(&maior, [[x + 40.0, y]]), bit(0));
    }

    #[test]
    fn o_tamanho_de_uma_peca_nao_mexe_nas_outras_e_tem_limite() {
        let mut tamanhos = BTreeMap::new();
        tamanhos.insert("botoes".to_string(), 150);
        tamanhos.insert("dpad".to_string(), 10);
        let elementos = monta(TELA, 100, &BTreeMap::new(), &tamanhos);
        let meio = |peca| elementos.iter().find(|e| e.peca == peca).unwrap().meio;
        assert_eq!(meio(Peca::B1), [45.0, 45.0]);
        assert_eq!(meio(Peca::B2), [45.0, 45.0]);
        assert_eq!(meio(Peca::Zl), [44.0, 22.0]);
        // Gravado abaixo do mínimo, desenhado no mínimo.
        assert_eq!(meio(Peca::Direcional), [36.0, 36.0]);
        // O losango cresce junto, mas o direcional, que é de outra chave, não saiu do lugar.
        let [x, _] = centro(&elementos, Peca::B4);
        let [cx, _] = centro_do_grupo(&elementos, Peca::B4).unwrap();
        assert_eq!(x - cx, 93.0);
        assert_eq!(
            centro(&elementos, Peca::Direcional),
            centro(&fabrica(), Peca::Direcional)
        );
        // E a escala geral multiplica a da peça.
        let maior = monta(TELA, 150, &BTreeMap::new(), &tamanhos);
        assert_eq!(maior.iter().find(|e| e.peca == Peca::B1).unwrap().meio, [67.5, 67.5]);
    }

    #[test]
    fn o_losango_inteiro_cabe_mesmo_gravado_no_canto() {
        let mut posicoes = BTreeMap::new();
        posicoes.insert("botoes".to_string(), [1.0, 1.0]);
        let elementos = monta(TELA, 100, &posicoes, &BTreeMap::new());
        // O 4 encosta na borda direita e o 1 na de baixo; o losango continua um losango.
        assert_eq!(centro(&elementos, Peca::B4), [TELA[0] - 30.0, TELA[1] - 92.0]);
        assert_eq!(centro(&elementos, Peca::B1), [TELA[0] - 92.0, TELA[1] - 30.0]);
    }

    #[test]
    fn o_editor_pega_a_peca_debaixo_do_dedo() {
        let elementos = fabrica();
        assert_eq!(debaixo(&elementos, centro(&elementos, Peca::Zr)), Some(Peca::Zr));
        assert_eq!(debaixo(&elementos, [TELA[0] / 2.0, TELA[1] / 3.0]), None);
    }
}
