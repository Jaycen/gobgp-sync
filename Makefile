# make package / make docker / make help
#   make package GOBGP_VERSION=v4.3.0 TARGET=x86_64-unknown-linux-musl
#   make docker PLATFORM=linux/arm64

APP         := gobgp-sync
VERSION     := $(shell sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)
HOST_TRIPLE := $(shell rustc -vV | sed -n 's/^host: //p')
TARGET      ?= $(HOST_TRIPLE)
PLATFORM    ?=
GOBGP_VERSION ?= latest
GOBGP_REPO  := https://github.com/osrg/gobgp.git

DIST_DIR    := dist
CACHE_DIR   := .cache/gobgp
SRC_DIR     := $(CACHE_DIR)/src

CARGO_TARGET_DIR ?= $(shell cargo metadata --format-version 1 --no-deps 2>/dev/null | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
ifeq ($(CARGO_TARGET_DIR),)
  CARGO_TARGET_DIR := target
endif

# aarch64-apple-darwin → darwin_arm64；x86_64-unknown-linux-musl → linux_amd64
GOOS   := $(shell echo $(TARGET) | awk '/darwin|apple/{print "darwin"; exit} /linux/{print "linux"; exit} {print "unsupported"}')
GOARCH := $(shell echo $(TARGET) | awk '/aarch64|arm64/{print "arm64"; exit} /x86_64|amd64/{print "amd64"; exit} {print "unsupported"}')
PAIR   := $(GOOS)_$(GOARCH)

STAGE_NAME := $(APP)_$(VERSION)_$(PAIR)
STAGE_DIR  := $(DIST_DIR)/$(STAGE_NAME)
TARBALL    := $(DIST_DIR)/$(STAGE_NAME).tar.gz
GOBGPD_BIN := $(CACHE_DIR)/gobgpd-$(PAIR)

ifeq ($(TARGET),$(HOST_TRIPLE))
  CARGO_FLAGS :=
  BIN_PATH    := $(CARGO_TARGET_DIR)/release/$(APP)
else
  CARGO_FLAGS := --target $(TARGET)
  BIN_PATH    := $(CARGO_TARGET_DIR)/$(TARGET)/release/$(APP)
endif

.PHONY: all help build build-gobgpd package docker clean check-target

all: package

help:
	@echo "make package   编译 $(APP)+gobgpd，打 tar.gz"
	@echo "make docker    镜像 $(APP):$(VERSION) 与 :latest"
	@echo "make build     仅编译 $(APP)"
	@echo "make clean     删除 dist/、.cache/、target/"
	@echo "TARGET=$(TARGET)  →  $(PAIR)    GOBGP_VERSION=$(GOBGP_VERSION)"
	@echo "PLATFORM=$(or $(PLATFORM),docker默认)"

check-target:
	@test "$(GOOS)" != unsupported -a "$(GOARCH)" != unsupported || { \
	  echo "无法从 TARGET=$(TARGET) 映射 GOOS/GOARCH"; exit 1; }

build: check-target
	cargo build --release $(CARGO_FLAGS)

build-gobgpd: check-target
	@command -v go >/dev/null || { echo "需要安装 Go"; exit 1; }
	@command -v git >/dev/null || { echo "需要安装 git"; exit 1; }
	@set -e; \
	mkdir -p $(CACHE_DIR); \
	if [ "$(GOBGP_VERSION)" = latest ]; then \
	  TAG=$$(git ls-remote --tags --refs $(GOBGP_REPO) \
	    | sed 's|.*/||' | grep -E '^v4\.[0-9]+\.[0-9]+$$' \
	    | sort -t. -k1.2,1n -k2,2n -k3,3n | tail -1); \
	  test -n "$$TAG" || { echo "无法解析 GoBGP 最新 v4 tag"; exit 1; }; \
	else \
	  TAG=$(GOBGP_VERSION); \
	  case $$TAG in v*) ;; *) TAG=v$$TAG ;; esac; \
	fi; \
	OUT=$(CACHE_DIR)/$$TAG-$(PAIR); \
	mkdir -p $$OUT; \
	if [ ! -x $$OUT/gobgpd ]; then \
	  if [ ! -d $(SRC_DIR)/.git ]; then git clone --filter=blob:none --no-checkout $(GOBGP_REPO) $(SRC_DIR); fi; \
	  git -C $(SRC_DIR) fetch --depth 1 origin refs/tags/$$TAG:refs/tags/$$TAG 2>/dev/null \
	    || git -C $(SRC_DIR) fetch --tags --force origin; \
	  git -C $(SRC_DIR) checkout -f $$TAG; \
	  OUT_ABS=$$(cd $$OUT && pwd); \
	  ( cd $(SRC_DIR) && CGO_ENABLED=0 GOOS=$(GOOS) GOARCH=$(GOARCH) \
	    go build -trimpath -ldflags='-s -w' -o $$OUT_ABS/gobgpd ./cmd/gobgpd ); \
	  chmod +x $$OUT/gobgpd; \
	fi; \
	ln -sfn $$TAG-$(PAIR)/gobgpd $(GOBGPD_BIN); \
	echo "gobgpd $$TAG -> $(GOBGPD_BIN)"

package: build build-gobgpd
	@test -x $(GOBGPD_BIN) || { echo "缺少 gobgpd: $(GOBGPD_BIN)"; exit 1; }
	@test -f $(BIN_PATH) || { echo "缺少编译产物: $(BIN_PATH)"; exit 1; }
	rm -rf $(STAGE_DIR)
	mkdir -p $(STAGE_DIR)/bin $(STAGE_DIR)/logs $(STAGE_DIR)/snapshot
	cp $(BIN_PATH) $(STAGE_DIR)/bin/$(APP)
	cp $(GOBGPD_BIN) $(STAGE_DIR)/bin/gobgpd
	cp -R config $(STAGE_DIR)/
	chmod +x $(STAGE_DIR)/bin/$(APP) $(STAGE_DIR)/bin/gobgpd
	tar -C $(DIST_DIR) -czf $(TARBALL) $(STAGE_NAME)
	@echo 已生成 $(TARBALL)
	@tar -tzf $(TARBALL)

docker:
	docker build $(if $(PLATFORM),--platform $(PLATFORM)) \
	  --build-arg GOBGP_VERSION=$(GOBGP_VERSION) \
	  -t $(APP):$(VERSION) -t $(APP):latest .
	@echo 已生成镜像 $(APP):$(VERSION) $(APP):latest $(PLATFORM)

clean:
	rm -rf $(DIST_DIR) .cache $(CARGO_TARGET_DIR)
