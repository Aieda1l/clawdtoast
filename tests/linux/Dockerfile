# Linux build + display test environment: Xvfb for the X11 overlay, headless sway (wlroots) for the
# Wayland layer-shell overlay. Used by tests/linux/run.sh; not needed for normal builds.
FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential pkg-config curl ca-certificates git libasound2-dev \
      fonts-dejavu-core procps python3 \
      xvfb x11-utils x11-apps xdotool openbox imagemagick \
      sway grim libnotify-bin dbus dbus-x11 \
    && rm -rf /var/lib/apt/lists/*
RUN apt-get update && apt-get install -y --no-install-recommends xcompmgr \
    && rm -rf /var/lib/apt/lists/*
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --component clippy
ENV PATH=/root/.cargo/bin:$PATH
# Runtime libraries winit's X11 backend dlopens.
RUN apt-get update && apt-get install -y --no-install-recommends \
      libx11-6 libx11-xcb1 libxcursor1 libxi6 libxrandr2 libxkbcommon0 libxkbcommon-x11-0 \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
