# SPDX-License-Identifier: GPL-2.0-only
obj-m := bt-kmod.o

bt-kmod-y := \
    src/core/bt_main.o \
    src/core/bt_capture.o \
    src/core/bt_hooks.o \
    src/core/bt_symbols.o \
    src/ipc/bt_control.o \
    src/ipc/bt_event.o \
    src/ipc/bt_protocol.o \
    src/ipc/bt_sock.o \
    src/hook/hijack_arm64.o \
    src/hook/inline_hook.o \
    src/utils/bt_kallsyms.o \
    src/utils/bt_mem.o \
    src/utils/bt_module.o \
    src/utils/bt_pgtable.o

src := $(if $(filter /%,$(src)),$(src),$(srctree)/$(src))

KDIR := $(KDIR)
MDIR := $(realpath $(dir $(abspath $(lastword $(MAKEFILE_LIST)))))

KDIR_BASENAME := $(notdir $(patsubst %/,%,$(KDIR)))
ANDROID_TAG := $(firstword $(subst -, ,$(KDIR_BASENAME)))
ANDROID_VER := $(strip $(patsubst android%,%,$(ANDROID_TAG)))

$(info -- KDIR: $(KDIR))
$(info -- MDIR: $(MDIR))
$(info -- BINDER_TRACE_SRC_DIR: $(src))
$(info -- BINDER_TRACE_OBJ_DIR: $(obj))
$(info -- KDIR_BASENAME=$(KDIR_BASENAME))
$(info -- ANDROID_TAG=$(ANDROID_TAG))
$(info -- ANDROID_VER=[$(ANDROID_VER)])

ccflags-y += -I$(src)/src/core
ccflags-y += -I$(src)/src/ipc
ccflags-y += -I$(src)/src/hook
ccflags-y += -I$(src)/src/utils

ccflags-y += -Wno-implicit-function-declaration -Wno-strict-prototypes -Wno-int-conversion -Wno-gcc-compat
ccflags-y += -Wno-declaration-after-statement -Wno-unused-function -Wno-unused-variable -Wno-unused-label

ifeq ($(ANDROID_VER),12)
  ccflags-y += -DANDROID_12
else
ifeq ($(ANDROID_VER),13)
  ccflags-y += -DANDROID_13
else
ifeq ($(ANDROID_VER),14)
  ccflags-y += -DANDROID_14
else
ifeq ($(ANDROID_VER),15)
  ccflags-y += -DANDROID_15
else
ifeq ($(ANDROID_VER),16)
  ccflags-y += -DANDROID_16
else
  $(error Unsupported ANDROID_VER: '$(ANDROID_VER)' from KDIR='$(KDIR)' basename='$(KDIR_BASENAME)')
endif
endif
endif
endif
endif

ccflags-y += -DINLINE_HOOK
ccflags-y += -DUSE_MULTI_INSN_PATCH

all:
	make -C $(KDIR) M=$(MDIR) modules

clean:
	make -C $(KDIR) M=$(MDIR) clean

compdb:
	python3 $(MDIR)/scripts/generate_compdb.py -O $(KDIR) $(MDIR)

.PHONY: all clean compdb
