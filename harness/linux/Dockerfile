# Environnement Linux de parité : PHP 8.4 + Composer 2 + Rust stable.
# Sert à exercer le chemin hardlink du clone et les chemins de cache Linux
# avant toute CI distante. Usage : harness/linux.sh
FROM php:8.4-cli-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends \
      git unzip jq curl ca-certificates build-essential pkg-config libicu-dev libzip-dev libpng-dev libjpeg62-turbo-dev \
    && docker-php-ext-configure gd --with-jpeg \
    && docker-php-ext-install intl bcmath zip exif gd \
    && rm -rf /var/lib/apt/lists/*
COPY --from=composer:2 /usr/bin/composer /usr/local/bin/composer
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --component rustfmt,clippy
ENV PATH="/root/.cargo/bin:${PATH}"
ENV COMPOSER_ALLOW_SUPERUSER=1
WORKDIR /work
