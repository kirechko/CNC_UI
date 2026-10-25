# syntax=docker/dockerfile:1
FROM mcr.microsoft.com/devcontainers/rust:1-bookworm AS base

# 1. Удаляем предустановленного пользователя vscode (он занимает UID/GID 1000)
#    и создаём своего пользователя с тем же UID/GID.
ARG USERNAME=kirechko
ARG USER_UID=1000
ARG USER_GID=$USER_UID

RUN userdel -r vscode 2>/dev/null || true && \
    groupdel vscode 2>/dev/null || true && \
    groupadd --gid $USER_GID $USERNAME && \
    useradd --uid $USER_UID --gid $USER_GID -m -s /bin/bash $USERNAME && \
    echo "$USERNAME ALL=(ALL) NOPASSWD:ALL" >> /etc/sudoers

# 2. Устанавливаем системные зависимости
# Убран python3-ensurepip (включен в python3-venv в Bookworm)
# Добавлен ca-certificates для корректной работы pip через HTTPS
RUN apt-get update && export DEBIAN_FRONTEND=noninteractive && \
    apt-get -y install --no-install-recommends \
       python3 python3-pip python3-venv python3-setuptools \
       ca-certificates curl wget sudo git ripgrep \
       pkg-config libglib2.0-dev libgtk-3-dev libssl-dev \
       xvfb x11-utils imagemagick scrot xdotool \
       libnss3 libatk1.0-0 libatk-bridge2.0-0 libcups2 libdrm2 \
       libxkbcommon0 libxcomposite1 libxdamage1 libxrandr2 libgbm1 \
       libpango-1.0-0 libcairo2 libasound2 libatspi2.0-0 libxshmfence1 && \
    rm -rf /var/lib/apt/lists/*

# 3. Устанавливаем Playwright
ENV PLAYWRIGHT_BROWSERS_PATH=/ms-playwright
RUN mkdir -p /ms-playwright && chmod 777 /ms-playwright && \
    pip3 install --break-system-packages playwright && \
    playwright install --with-deps chromium

# 4. Создаем venv для MCP (pip включён по умолчанию)
RUN mkdir -p /home/$USERNAME && \
    chown $USERNAME:$USERNAME /home/$USERNAME && \
    chmod 755 /home/$USERNAME && \
    python3 -m venv /home/$USERNAME/mcp-env && \
    /home/$USERNAME/mcp-env/bin/pip install --no-cache-dir --upgrade pip && \
    /home/$USERNAME/mcp-env/bin/pip install --no-cache-dir mcp && \
    chown -R $USERNAME:$USERNAME /home/$USERNAME/mcp-env
    
# 5. Переключаемся на пользователя и настраиваем окружение
USER $USERNAME
ENV PATH="/home/$USERNAME/mcp-env/bin:$PATH"
ENV DISPLAY=:99
WORKDIR /workspaces/CNC_UI_Dev
