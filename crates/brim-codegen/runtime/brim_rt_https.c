/*
 * HTTPS requests for `std.http`, through libcurl. Compiled and linked
 * (with -lcurl) only into programs that make them, so other programs do not
 * need libcurl. The interpreter makes the same calls through the `curl`
 * crate (brim-interp/src/https.rs), with the same options and error texts.
 */

#include <curl/curl.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

typedef struct BrimStr {
    int64_t rc;
    int64_t len;
    char data[];
} BrimStr;

void brim_rt_internal_set_error(const char *reason);
BrimStr *brim_rt_internal_str(const char *data, int64_t len);
int brim_rt_internal_valid_utf8(const char *data, int64_t len);
char *brim_rt_internal_c_string(const BrimStr *s);

typedef struct Bytes {
    char *data;
    size_t len;
    size_t cap;
} Bytes;

static void bytes_add(Bytes *b, const char *data, size_t len) {
    if (b->len + len > b->cap) {
        size_t cap = (b->len + len) * 2 + 256;
        char *grown = realloc(b->data, cap);
        if (grown == NULL) {
            abort();
        }
        b->data = grown;
        b->cap = cap;
    }
    memcpy(b->data + b->len, data, len);
    b->len += len;
}

static size_t on_body(char *data, size_t size, size_t count, void *user) {
    bytes_add((Bytes *)user, data, size * count);
    return size * count;
}

/* Keeps the header lines of the last response (a 100 Continue or a
 * redirect starts a new one). */
static size_t on_header(char *data, size_t size, size_t count, void *user) {
    Bytes *b = (Bytes *)user;
    size_t n = size * count;
    if (n >= 5 && memcmp(data, "HTTP/", 5) == 0) {
        b->len = 0;
    } else if (!(n == 2 && data[0] == '\r') && !(n == 1 && data[0] == '\n')) {
        bytes_add(b, data, n);
    }
    return n;
}

static const char *reason(CURLcode code) {
    switch (code) {
    case CURLE_COULDNT_RESOLVE_HOST: return "host not found";
    case CURLE_COULDNT_CONNECT: return "connection refused";
    case CURLE_OPERATION_TIMEDOUT: return "timed out";
    case CURLE_PEER_FAILED_VERIFICATION:
#if LIBCURL_VERSION_NUM < 0x073e00
    case CURLE_SSL_CACERT:
#endif
    case CURLE_SSL_CACERT_BADFILE: return "certificate not trusted";
    case CURLE_SSL_CONNECT_ERROR: return "TLS handshake failed";
    case CURLE_URL_MALFORMAT:
    case CURLE_UNSUPPORTED_PROTOCOL: return "invalid URL";
    case CURLE_GOT_NOTHING:
    case CURLE_RECV_ERROR:
    case CURLE_SEND_ERROR: return "connection reset";
    default: return "network error";
    }
}

BrimStr *brim_rt_http_request(const BrimStr *method, const BrimStr *url, const BrimStr *headers,
                              const BrimStr *body, int64_t timeout_ms) {
    brim_rt_internal_set_error(NULL);
    static int ready;
    if (!ready) {
        curl_global_init(CURL_GLOBAL_DEFAULT);
        ready = 1;
    }
    CURL *curl = curl_easy_init();
    if (curl == NULL) {
        brim_rt_internal_set_error("network error");
        return brim_rt_internal_str(NULL, 0);
    }
    char *c_method = brim_rt_internal_c_string(method);
    char *c_url = brim_rt_internal_c_string(url);
    char *c_headers = brim_rt_internal_c_string(headers);
    struct curl_slist *list = curl_slist_append(NULL, "Expect:");
    for (char *line = strtok(c_headers, "\r\n"); line != NULL; line = strtok(NULL, "\r\n")) {
        list = curl_slist_append(list, line);
    }
    Bytes head = {0};
    Bytes content = {0};
    curl_easy_setopt(curl, CURLOPT_URL, c_url);
    curl_easy_setopt(curl, CURLOPT_CUSTOMREQUEST, c_method);
    curl_easy_setopt(curl, CURLOPT_HTTPHEADER, list);
    curl_easy_setopt(curl, CURLOPT_NOSIGNAL, 1L);
    curl_easy_setopt(curl, CURLOPT_FOLLOWLOCATION, 0L);
    curl_easy_setopt(curl, CURLOPT_TIMEOUT_MS, (long)timeout_ms);
    if (strcmp(c_method, "HEAD") == 0) {
        curl_easy_setopt(curl, CURLOPT_NOBODY, 1L);
    }
    if (body->len > 0 || strcmp(c_method, "POST") == 0 || strcmp(c_method, "PUT") == 0 ||
        strcmp(c_method, "PATCH") == 0) {
        curl_easy_setopt(curl, CURLOPT_POSTFIELDSIZE_LARGE, (curl_off_t)body->len);
        curl_easy_setopt(curl, CURLOPT_POSTFIELDS, body->data);
    }
    curl_easy_setopt(curl, CURLOPT_WRITEFUNCTION, on_body);
    curl_easy_setopt(curl, CURLOPT_WRITEDATA, &content);
    curl_easy_setopt(curl, CURLOPT_HEADERFUNCTION, on_header);
    curl_easy_setopt(curl, CURLOPT_HEADERDATA, &head);
    CURLcode code = curl_easy_perform(curl);
    long status = 0;
    curl_easy_getinfo(curl, CURLINFO_RESPONSE_CODE, &status);
    curl_easy_cleanup(curl);
    curl_slist_free_all(list);
    free(c_method);
    free(c_url);
    free(c_headers);

    BrimStr *result;
    if (code != CURLE_OK) {
        brim_rt_internal_set_error(reason(code));
        result = brim_rt_internal_str(NULL, 0);
    } else if (!brim_rt_internal_valid_utf8(content.data, (int64_t)content.len)) {
        brim_rt_internal_set_error("the response is not valid UTF-8");
        result = brim_rt_internal_str(NULL, 0);
    } else {
        Bytes out = {0};
        char status_text[32];
        int n = snprintf(status_text, sizeof status_text, "%ld\r\n", status);
        bytes_add(&out, status_text, (size_t)n);
        if (head.len > 0) {
            bytes_add(&out, head.data, head.len);
        }
        bytes_add(&out, "\r\n", 2);
        if (content.len > 0) {
            bytes_add(&out, content.data, content.len);
        }
        result = brim_rt_internal_str(out.data, (int64_t)out.len);
        free(out.data);
    }
    free(head.data);
    free(content.data);
    return result;
}
