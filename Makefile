COMPILER_IMG ?= tommylike/rust_linux_compile:0.0.1
CURRENT_DIR = $(shell pwd)

linux-image:
	docker build ./tools -t ${COMPILER_IMG} -f ./tools/Dockerfile.linux

linux-binary: linux-image
	docker run -v ${CURRENT_DIR}:/app --rm ${COMPILER_IMG}
