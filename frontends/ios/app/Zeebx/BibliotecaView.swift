import AVFoundation
import SwiftUI

/// O estado que a lista e o jogo compartilham.
///
/// O ponteiro é o que `zeebx_ios_cria` devolveu. Ele vive o tempo do processo: a lista e o
/// quadro são a mesma sessão, e destruí-lo no meio de uma troca de tela deixaria a outra com
/// um ponteiro morto.
final class ZeebxModelo: ObservableObject {
    let app: OpaquePointer
    @Published var titulos: [String] = []
    @Published var jogando = false
    @Published var abrindo = false
    @Published var erro: String?

    init() {
        let documentos = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        let suporte = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        try? FileManager.default.createDirectory(at: suporte, withIntermediateDirectories: true)
        let criado = documentos.path.withCString { docs in
            suporte.path.withCString { sup in
                zeebx_ios_cria(docs, sup)
            }
        }
        guard let criado else {
            fatalError("zeebx_ios_cria devolveu nulo com pastas do sandbox")
        }
        app = criado
        varre()
    }

    deinit {
        zeebx_ios_destroi(app)
    }

    func varre() {
        let quantidade = zeebx_ios_varre(app)
        titulos = (0..<quantidade).compactMap { indice in
            guard let ponteiro = zeebx_ios_titulo(app, indice) else { return nil }
            return String(cString: ponteiro)
        }
    }

    func abre(_ indice: Int) {
        abrindo = true
        // A abertura do módulo trava a linha da interface. Este salto deixa o indicador
        // aparecer antes; a sessão em si continua na linha principal, que é a única que
        // mexe no ponteiro.
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            let audio = AVAudioSession.sharedInstance()
            try? audio.setCategory(.playback, mode: .default)
            try? audio.setActive(true)
            if zeebx_ios_comeca(self.app, Int32(indice)) == 0 {
                self.erro = nil
                self.jogando = true
            } else if let ponteiro = zeebx_ios_erro(self.app) {
                self.erro = String(cString: ponteiro)
            }
            self.abrindo = false
        }
    }

    func fecha() {
        zeebx_ios_para(app)
        jogando = false
    }
}

struct BibliotecaView: View {
    @ObservedObject var modelo: ZeebxModelo

    var body: some View {
        NavigationStack {
            Group {
                if modelo.titulos.isEmpty {
                    VStack(spacing: 12) {
                        Text("Nenhum jogo em Documents/roms")
                            .font(.title2)
                        Text("Pelo app Arquivos, ou pelo Finder com o aparelho ligado, coloque os .mod ou .zip nessa pasta e volte aqui.")
                            .multilineTextAlignment(.center)
                            .foregroundStyle(.secondary)
                            .padding(.horizontal, 32)
                    }
                } else {
                    List {
                        ForEach(Array(modelo.titulos.enumerated()), id: \.offset) { indice, titulo in
                            Button(titulo) {
                                modelo.abre(indice)
                            }
                            .disabled(modelo.abrindo)
                        }
                    }
                }
            }
            .navigationTitle("Zeebx")
            .toolbar {
                Button("Atualizar") { modelo.varre() }
            }
            .overlay {
                if modelo.abrindo {
                    ProgressView("Abrindo")
                        .padding(24)
                        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
                }
            }
        }
        .fullScreenCover(isPresented: $modelo.jogando) {
            JogoView(modelo: modelo)
        }
        .alert("Não deu para abrir", isPresented: erroVisivel) {
            Button("OK", role: .cancel) { modelo.erro = nil }
        } message: {
            Text(modelo.erro ?? "")
        }
    }

    private var erroVisivel: Binding<Bool> {
        Binding(
            get: { modelo.erro != nil && !modelo.jogando },
            set: { if !$0 { modelo.erro = nil } }
        )
    }
}
