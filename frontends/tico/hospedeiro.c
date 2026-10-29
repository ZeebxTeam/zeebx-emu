/* O hospedeiro do módulo. O laço do jogo é Rust; aqui ficam o que o libnx não
 * entrega por outra língua: framebuffer, audout, controle, o alias de JIT que o
 * dynarmic pede e a volta ao tico.
 *
 * O patch do dynarmic chama switch_spinlock_lock sem pôr o ponteiro do cadeado
 * em X0 — o BLR sai com o que já estava no registrador. Por isso a trava é uma
 * só, global, e o argumento se ignora. */
#include <switch.h>

#include <malloc.h>
#include <stdatomic.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

typedef struct {
	uint64_t botoes;
	int32_t lx, ly, rx, ry;
} ZeebxLeitura;

typedef struct {
	void (*apresenta)(const uint16_t *pixels, uint32_t largura, uint32_t altura);
	void (*audio)(const int16_t *pcm, uint32_t quadros);
	void (*entrada)(ZeebxLeitura *saida);
	void (*erro)(const char *mensagem);
} ZeebxHospedeiro;

int zeebx_tico_roda(const char *rom, const char *titulo, const char *saves, const char *states,
	const char *system, const char *roms, const char *config, const ZeebxHospedeiro *hospedeiro);

void zeebx_copia_rgb565(uint16_t *destino, uint32_t passo_bytes, uint32_t dest_w, uint32_t dest_h,
	const uint16_t *origem, uint32_t origem_w, uint32_t origem_h);

#define BLOCOS 8
static Jit blocos[BLOCOS];
static bool bloco_usado[BLOCOS];

static Jit *acha(void *p) {
	if (!p) {
		return NULL;
	}
	for (int i = 0; i < BLOCOS; i++) {
		if (!bloco_usado[i]) {
			continue;
		}
		if (blocos[i].rw_addr == p || blocos[i].rx_addr == p) {
			return &blocos[i];
		}
	}
	return NULL;
}

void *switch_jit_create(size_t size, void **rx) {
	if (size == 0) {
		return NULL;
	}
	size = (size + 0xFFF) & ~(size_t)0xFFF;
	for (int i = 0; i < BLOCOS; i++) {
		if (bloco_usado[i]) {
			continue;
		}
		memset(&blocos[i], 0, sizeof(Jit));
		if (R_FAILED(jitCreate(&blocos[i], size))) {
			return NULL;
		}
		bloco_usado[i] = true;
		if (rx) {
			*rx = jitGetRxAddr(&blocos[i]);
		}
		return jitGetRwAddr(&blocos[i]);
	}
	return NULL;
}

void switch_jit_destroy(void *rw) {
	Jit *j = acha(rw);
	if (!j) {
		return;
	}
	jitClose(j);
	bloco_usado[j - blocos] = false;
}

void switch_jit_writable(void *rw) {
	Jit *j = acha(rw);
	if (j) {
		jitTransitionToWritable(j);
	}
}

void switch_jit_executable(void *rw) {
	Jit *j = acha(rw);
	if (j) {
		jitTransitionToExecutable(j);
	}
}

/* A escrita foi no alias RW e a execução lê o RX. Limpar só o ponteiro que o
 * dynarmic passou deixa o outro alias com a linha de cache velha. */
void switch_jit_flush(void *addr, size_t size) {
	if (!addr || size == 0) {
		return;
	}
	Jit *j = acha(addr);
	if (!j) {
		armDCacheFlush(addr, size);
		armICacheInvalidate(addr, size);
		return;
	}
	uintptr_t onde = (uintptr_t)addr;
	uintptr_t base_rx = (uintptr_t)j->rx_addr;
	uintptr_t base_rw = (uintptr_t)j->rw_addr;
	size_t off = 0;
	if (onde >= base_rx && onde < base_rx + j->size) {
		off = onde - base_rx;
	} else if (onde >= base_rw && onde < base_rw + j->size) {
		off = onde - base_rw;
	}
	if (off + size > j->size) {
		size = j->size - off;
	}
	armDCacheFlush((uint8_t *)j->rw_addr + off, size);
	armICacheInvalidate((uint8_t *)j->rx_addr + off, size);
}

static Mutex trava_global;

void switch_spinlock_lock(volatile int *cadeado) {
	(void)cadeado;
	mutexLock(&trava_global);
}

void switch_spinlock_unlock(volatile int *cadeado) {
	(void)cadeado;
	mutexUnlock(&trava_global);
}

#define QUADROS_AUDIO 1024
#define BYTES_AUDIO (QUADROS_AUDIO * 2 * (int)sizeof(int16_t))
#define ANEL_CAP 96000

static int16_t anel[ANEL_CAP];
static uint32_t anel_usados;
static uint32_t anel_le;
static Mutex anel_trava;
static atomic_bool audio_viva;
static bool audio_ok;
static Thread fio_audio;
static AudioOutBuffer saidas[2];
static int16_t *amostras[2];

static void empurra_som(const int16_t *pcm, uint32_t quadros) {
	if (!audio_ok || !pcm || quadros == 0) {
		return;
	}
	uint32_t n = quadros * 2;
	mutexLock(&anel_trava);
	uint32_t livres = ANEL_CAP - anel_usados;
	if (n > livres) {
		n = livres & ~1u;
	}
	uint32_t escreve = (anel_le + anel_usados) % ANEL_CAP;
	for (uint32_t i = 0; i < n; i++) {
		anel[escreve] = pcm[i];
		escreve++;
		if (escreve == ANEL_CAP) {
			escreve = 0;
		}
	}
	anel_usados += n;
	mutexUnlock(&anel_trava);
}

static void puxa_som(int16_t *destino, uint32_t n) {
	mutexLock(&anel_trava);
	uint32_t tem = anel_usados < n ? anel_usados : n;
	tem &= ~1u;
	for (uint32_t i = 0; i < tem; i++) {
		destino[i] = anel[anel_le];
		anel_le++;
		if (anel_le == ANEL_CAP) {
			anel_le = 0;
		}
	}
	anel_usados -= tem;
	mutexUnlock(&anel_trava);
	if (tem < n) {
		memset(destino + tem, 0, (n - tem) * sizeof(int16_t));
	}
}

static void preenche(AudioOutBuffer *saida, int16_t *buf) {
	puxa_som(buf, QUADROS_AUDIO * 2);
	saida->next = NULL;
	saida->buffer = buf;
	saida->buffer_size = BYTES_AUDIO;
	saida->data_size = BYTES_AUDIO;
	saida->data_offset = 0;
}

static void fio_do_som(void *arg) {
	(void)arg;
	preenche(&saidas[0], amostras[0]);
	preenche(&saidas[1], amostras[1]);
	audoutAppendAudioOutBuffer(&saidas[0]);
	audoutAppendAudioOutBuffer(&saidas[1]);
	audoutStartAudioOut();
	int i = 0;
	while (atomic_load(&audio_viva)) {
		AudioOutBuffer *solto = NULL;
		u32 quantos = 0;
		audoutWaitPlayFinish(&solto, &quantos, 100000000ULL);
		preenche(&saidas[i], amostras[i]);
		audoutAppendAudioOutBuffer(&saidas[i]);
		i ^= 1;
	}
}

static bool sobe_som(void) {
	if (R_FAILED(audoutInitialize())) {
		return false;
	}
	for (int i = 0; i < 2; i++) {
		amostras[i] = memalign(0x1000, BYTES_AUDIO);
		if (!amostras[i]) {
			for (int j = 0; j < i; j++) {
				free(amostras[j]);
				amostras[j] = NULL;
			}
			audoutExit();
			return false;
		}
		memset(amostras[i], 0, BYTES_AUDIO);
	}
	atomic_store(&audio_viva, true);
	if (R_FAILED(threadCreate(&fio_audio, fio_do_som, NULL, NULL, 0x8000, 0x20, -2))
		|| R_FAILED(threadStart(&fio_audio))) {
		atomic_store(&audio_viva, false);
		audoutExit();
		return false;
	}
	return true;
}

static void desce_som(void) {
	if (!audio_ok) {
		return;
	}
	atomic_store(&audio_viva, false);
	threadWaitForExit(&fio_audio);
	threadClose(&fio_audio);
	audoutStopAudioOut();
	audoutExit();
	free(amostras[0]);
	free(amostras[1]);
	audio_ok = false;
}

static Framebuffer fb;
static bool fb_ok;
static u32 fb_w = 1280;
static u32 fb_h = 720;
static PadState pad;
static bool esperando_erro;

static void apresenta(const uint16_t *pixels, uint32_t largura, uint32_t altura) {
	if (!fb_ok || !pixels) {
		return;
	}
	u32 passo = 0;
	u16 *destino = framebufferBegin(&fb, &passo);
	if (!destino) {
		return;
	}
	zeebx_copia_rgb565(destino, passo, fb_w, fb_h, pixels, largura, altura);
	framebufferEnd(&fb);
}

static void entrada(ZeebxLeitura *saida) {
	if (!saida) {
		return;
	}
	padUpdate(&pad);
	HidAnalogStickState esquerdo = padGetStickPos(&pad, 0);
	HidAnalogStickState direito = padGetStickPos(&pad, 1);
	saida->botoes = padGetButtons(&pad);
	saida->lx = esquerdo.x;
	saida->ly = esquerdo.y;
	saida->rx = direito.x;
	saida->ry = direito.y;
}

static void pinta_erro(void) {
	if (!fb_ok) {
		return;
	}
	u32 passo = 0;
	u16 *destino = framebufferBegin(&fb, &passo);
	if (!destino) {
		return;
	}
	u32 pixels = (passo / 2) * fb_h;
	for (u32 i = 0; i < pixels; i++) {
		destino[i] = RGB565(24, 4, 4);
	}
	framebufferEnd(&fb);
}

static void erro(const char *mensagem) {
	(void)mensagem;
	esperando_erro = true;
	pinta_erro();
}

static void volta_ao_tico(void) {
	const char *caminhos[] = {
		"sdmc:/switch/tico.nro",
		"sdmc:/switch/tico/tico.nro",
	};
	for (size_t i = 0; i < sizeof(caminhos) / sizeof(caminhos[0]); i++) {
		if (access(caminhos[i], F_OK) == 0) {
			/* O espaço antes de --resume é o que o guia do tico pede: o
			 * carregador junta o caminho, e o resto da linha começa aí. */
			envSetNextLoad(caminhos[i], " --resume");
			return;
		}
	}
}

int main(int argc, char **argv) {
	mutexInit(&trava_global);
	mutexInit(&anel_trava);
	padConfigureInput(1, HidNpadStyleSet_NpadStandard);
	padInitializeDefault(&pad);

	NWindow *janela = nwindowGetDefault();
	u32 largura = 1280;
	u32 altura = 720;
	if (janela && R_SUCCEEDED(nwindowGetDimensions(janela, &largura, &altura)) && largura > 0
		&& altura > 0) {
		fb_w = largura;
		fb_h = altura;
	}
	if (R_SUCCEEDED(framebufferCreate(&fb, janela, fb_w, fb_h, PIXEL_FORMAT_RGB_565, 2))
		&& R_SUCCEEDED(framebufferMakeLinear(&fb))) {
		fb_ok = true;
	}

	audio_ok = sobe_som();

	const char *rom = argc > 1 ? argv[1] : NULL;
	const char *titulo = argc > 2 ? argv[2] : NULL;
	const char *saves = argc > 3 ? argv[3] : NULL;
	const char *states = argc > 4 ? argv[4] : NULL;
	const char *system = argc > 5 ? argv[5] : NULL;
	const char *roms = argc > 6 ? argv[6] : NULL;
	const ZeebxHospedeiro hospedeiro = {
		.apresenta = apresenta,
		.audio = empurra_som,
		.entrada = entrada,
		.erro = erro,
	};
	int rc = zeebx_tico_roda(rom, titulo, saves, states, system, roms,
		"sdmc:/tico/config/cores/zeebx.jsonc", &hospedeiro);

	if (esperando_erro) {
		/* A tela vermelha não tem texto. O motivo está em zeebx.log. Um botão
		 * confirma que a pessoa viu, e aí sim o tico volta. */
		for (;;) {
			padUpdate(&pad);
			if (padGetButtonsDown(&pad) & (HidNpadButton_A | HidNpadButton_B | HidNpadButton_Plus)) {
				break;
			}
			svcSleepThread(10 * 1000 * 1000);
		}
	}

	desce_som();
	if (fb_ok) {
		framebufferClose(&fb);
	}
	volta_ao_tico();
	return rc;
}
