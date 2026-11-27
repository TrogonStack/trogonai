#!/bin/sh
# Stands in for the Python interpreter: argv is `<script> <op>`, the request arrives on stdin, and the sandbox id
# in the request picks the behavior.
op="$2"
input=$(cat)
case "$input" in
  *sb-crash*) echo "the bridge fell over" >&2; exit 3 ;;
  *sb-silent*) exit 0 ;;
  *sb-garbage*) echo "not json"; exit 0 ;;
  *sb-notfound*) echo '{"ok":false,"error":{"kind":"NotFoundError","message":"gone"}}'; exit 0 ;;
  *sb-auth*) echo '{"ok":false,"error":{"kind":"AuthError","message":"Token missing"}}'; exit 0 ;;
  *sb-invalid*) echo '{"ok":false,"error":{"kind":"InvalidError","message":"bad request"}}'; exit 0 ;;
  *sb-nodetail*) echo '{"ok":false}'; exit 0 ;;
  *sb-weird*) echo '{"ok":false,"error":{"kind":"QuotaError","message":"too many"}}'; exit 0 ;;
  *sb-mismatch*) echo '{"ok":true,"result":{"unexpected":1}}'; exit 0 ;;
esac
echo "warming up"
case "$op" in
  create|get) echo '{"ok":true,"result":{"id":"sb-1","state":"running","exit_code":null}}' ;;
  exec) echo '{"ok":true,"result":{"stdout":"out","stderr":"err","exit_code":0}}' ;;
  tunnels) echo '{"ok":true,"result":{"tunnels":[{"port":8080,"host":"h.modal.host","tls_port":443,"url":"https://h.modal.host"}]}}' ;;
  snapshot) echo '{"ok":true,"result":{"image_id":"im-9"}}' ;;
  *) echo '{"ok":true,"result":{}}' ;;
esac
echo ""
