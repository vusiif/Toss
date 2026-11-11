# The Linux release builder.
#
# One `toss` binary has to start on every mainstream distribution, so it is
# linked against the oldest glibc Toss supports: 2.28 (RHEL/Alma/Rocky 8,
# Debian 10, Ubuntu 20.04 and everything newer, Arch and Fedora included).
# glibc is backward compatible only — a binary built on a newer glibc refuses
# to start on an older one — so the build itself runs on an EL8 userland.
#
# gcc-toolset gives a current C/C++ compiler for the vendored native
# libraries while still targeting the system glibc 2.28.
# NASM assembles FFmpeg's x86 decode routines (about twice the speed); on
# EL8 it lives in the PowerTools repository.
FROM almalinux:8

RUN dnf -y install dnf-plugins-core \
 && dnf config-manager --set-enabled powertools \
 && dnf -y install gcc-toolset-13-gcc gcc-toolset-13-gcc-c++ cmake make \
        git perl diffutils file binutils nasm \
 && dnf clean all

ENV PATH=/opt/rh/gcc-toolset-13/root/usr/bin:/root/.cargo/bin:$PATH \
    CC=gcc CXX=g++

# Toss follows the latest stable Rust; the builder does the same.
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable \
 && rustc --version

WORKDIR /src
