# Single-node OpenBao for local development only.
#
# Two settings here are deliberately unsafe outside a laptop: the listener
# serves plaintext HTTP, and `seal "static"` keeps the root key under a
# checked-in symmetric key so the server auto-unseals on every restart. Neither
# is acceptable for a deployment that holds real key material.

ui            = true
disable_mlock = true
api_addr      = "http://openbao:8200"

listener "tcp" {
  address     = "0.0.0.0:8200"
  tls_disable = "true"
}

# `connection_url` comes from BAO_PG_CONNECTION_URL so the credentials stay in
# compose alongside the postgres service that owns them.
storage "postgresql" {
  ha_enabled = "false"
}

seal "static" {
  current_key_id = "local-dev"
  current_key    = "env://BAO_STATIC_SEAL_KEY"
}
