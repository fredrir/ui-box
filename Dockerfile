FROM debian:bookworm-slim

ARG TARGETARCH

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates \
      git \
      openssh-client \
      rsync \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --create-home --uid 1000 uibox

COPY --chmod=0755 image/linux/${TARGETARCH}/ui-box     /usr/local/bin/ui-box
COPY --chmod=0755 image/linux/${TARGETARCH}/ui-box-mcp /usr/local/bin/ui-box-mcp

USER uibox
WORKDIR /work

ENV UIBOX_ARTIFACTS=/work/.uibox/runs

ENTRYPOINT ["ui-box"]
CMD ["doctor"]
