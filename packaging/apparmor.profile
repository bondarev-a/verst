# Ubuntu 24.04 запрещает непривилегированные user namespace.
# Песочница WebKit без этого не стартует и роняет процесс.
# Профиль только разрешает namespace, сам процесс остаётся свободным.
abi <abi/4.0>,
include <tunables/global>

profile verst /usr/bin/verst flags=(unconfined) {
  userns,

  include if exists <local/verst>
}
