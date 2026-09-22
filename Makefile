# Núcleo Libretro do Zeebx, nos três sistemas.
#
#   make
#   make install
#
# Windows (PowerShell, sem Make):
#
#   .\build-libretro.ps1
#
# O `install` copia o núcleo e o `.info` para o RetroArch do usuário.

TARGET_NAME := zeebx_libretro
CARGO ?= cargo
PROFILE ?= release

ifeq ($(OS),Windows_NT)
  SOEXT := dll
  LIBNAME := zeebx.dll
  PREFIX ?= $(USERPROFILE)/AppData/Roaming/RetroArch
else
  UNAME_S := $(shell uname -s)
  ifeq ($(UNAME_S),Darwin)
    SOEXT := dylib
    LIBNAME := libzeebx.dylib
    PREFIX ?= $(HOME)/Library/Application Support/RetroArch
  else
    SOEXT := so
    LIBNAME := libzeebx.so
    PREFIX ?= $(HOME)/.config/retroarch
  endif
endif

CORE := $(TARGET_NAME).$(SOEXT)

ifeq ($(PROFILE),release)
  CARGO_FLAGS := --release
  OUTDIR := target/release
else
  OUTDIR := target/debug
endif

.PHONY: all clean install

all: $(CORE)

$(CORE):
	$(CARGO) build --lib $(CARGO_FLAGS)
	cp "$(OUTDIR)/$(LIBNAME)" "$(CORE)"

clean:
	$(CARGO) clean
	rm -f zeebx_libretro.so zeebx_libretro.dll zeebx_libretro.dylib

install: $(CORE)
	mkdir -p "$(PREFIX)/cores" "$(PREFIX)/info"
	cp "$(CORE)" "$(PREFIX)/cores/"
	cp zeebx_libretro.info "$(PREFIX)/info/"
