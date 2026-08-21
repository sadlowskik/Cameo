# Print the console URL and key on interactive logins.
case "$-" in *i*) ;; *) return 0 ;; esac
[ -t 1 ] || return 0
[ -x /usr/local/bin/cameo-hello ] || return 0
CAMEO_HELLO_WAIT=0 /usr/local/bin/cameo-hello
