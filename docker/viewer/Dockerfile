# 3D viewerの撮影専用のimage。Chromiumのheadless modeで、書き出したローカルのHTMLだけを開く。
# `docker run --network none` で使い、外部のweb contentを読まない。
#
# base imageはdigestで、packageはsnapshot.debian.orgの時刻で固定する。aptはimage内の
# Debianの鍵でRelease署名を検証し、各fileはReleaseのhashと照合する。取得はDebianの既定と
# 同じくHTTPで行う。slim imageはCA証明書を持たず、内容の真正性は署名で確かめるためである。
# security archiveのReleaseは7日で期限が切れるため、固定した時刻の内容を使うには
# 期限の検査を外す必要がある。
FROM debian:trixie-20260918-slim@sha256:a99cfc517144bc59b1978475ec53b46ecabec7e43635402ee5b77cc54cd1b20a

ARG SNAPSHOT_DEBIAN=20260930T203251Z
ARG SNAPSHOT_SECURITY=20260929T215738Z
ARG CHROMIUM_VERSION=154.0.8037.57-1~deb13u1

RUN rm -f /etc/apt/sources.list.d/debian.sources \
 && printf '%s\n' \
      "Types: deb" \
      "URIs: http://snapshot.debian.org/archive/debian/${SNAPSHOT_DEBIAN}/" \
      "Suites: trixie trixie-updates" \
      "Components: main" \
      "Signed-By: /usr/share/keyrings/debian-archive-keyring.gpg" \
      "Check-Valid-Until: no" \
      "" \
      "Types: deb" \
      "URIs: http://snapshot.debian.org/archive/debian-security/${SNAPSHOT_SECURITY}/" \
      "Suites: trixie-security" \
      "Components: main" \
      "Signed-By: /usr/share/keyrings/debian-archive-keyring.gpg" \
      "Check-Valid-Until: no" \
      > /etc/apt/sources.list.d/snapshot.sources \
 && apt-get update \
 && apt-get install --yes --no-install-recommends "chromium=${CHROMIUM_VERSION}" fonts-dejavu-core \
 && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home viewer
USER viewer
WORKDIR /home/viewer

# containerの中ではChromiumのsandboxに必要なuser namespaceを使えない。隔離はcontainerと
# network無しの実行で行う。WebGLはGPUを使わずSwiftShaderで描く。
ENTRYPOINT ["chromium", "--headless", "--no-sandbox", "--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--hide-scrollbars", "--disable-dev-shm-usage"]
