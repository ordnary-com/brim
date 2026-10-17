/*
 * HTTPS requests for `std.http`, through libcurl (WinHTTP on Windows).
 * Compiled and linked (with -lcurl or -lwinhttp) only into programs that
 * make them, so other programs do not need libcurl. The interpreter makes the same calls through the `curl`
 * crate (brim-interp/src/https.rs), with the same options and error texts.
 */

#ifdef _WIN32
/* Windows: the same requests through WinHTTP, the system's own HTTP
 * client, with the same result and error texts as the libcurl version
 * below. */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <winhttp.h>
#include <stdint.h>
#include <stdio.h>
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
    if (len > 0) {
        memcpy(b->data + b->len, data, len);
    }
    b->len += len;
}

/* UTF-8 bytes as a NUL-terminated wide string; the caller frees it. */
static wchar_t *wide(const char *data, int64_t len) {
    int n = len > 0 ? MultiByteToWideChar(CP_UTF8, 0, data, (int)len, NULL, 0) : 0;
    wchar_t *out = malloc(sizeof(wchar_t) * (size_t)(n + 1));
    if (out == NULL) {
        abort();
    }
    if (n > 0) {
        MultiByteToWideChar(CP_UTF8, 0, data, (int)len, out, n);
    }
    out[n] = 0;
    return out;
}

static const char *reason(DWORD error) {
    switch (error) {
    case ERROR_WINHTTP_NAME_NOT_RESOLVED: return "host not found";
    case ERROR_WINHTTP_CANNOT_CONNECT: return "connection refused";
    case ERROR_WINHTTP_TIMEOUT: return "timed out";
    case ERROR_WINHTTP_SECURE_FAILURE:
    case ERROR_WINHTTP_SECURE_INVALID_CA:
    case ERROR_WINHTTP_SECURE_CERT_CN_INVALID:
    case ERROR_WINHTTP_SECURE_CERT_DATE_INVALID:
    case ERROR_WINHTTP_SECURE_CERT_REVOKED:
    case ERROR_WINHTTP_SECURE_INVALID_CERT: return "certificate not trusted";
    case ERROR_WINHTTP_SECURE_CHANNEL_ERROR: return "TLS handshake failed";
    case ERROR_WINHTTP_INVALID_URL:
    case ERROR_WINHTTP_UNRECOGNIZED_SCHEME: return "invalid URL";
    case ERROR_WINHTTP_CONNECTION_ERROR: return "connection reset";
    default: return "network error";
    }
}

/* Whether a Brim string differs from `text`; 0 when equal. */
static int strcmp_len(const BrimStr *s, const char *text) {
    size_t n = strlen(text);
    return !((size_t)s->len == n && memcmp(s->data, text, n) == 0);
}

static BrimStr *fail(const char *why) {
    brim_rt_internal_set_error(why);
    return brim_rt_internal_str(NULL, 0);
}

BrimStr *brim_rt_http_request(const BrimStr *method, const BrimStr *url, const BrimStr *headers,
                              const BrimStr *body, int64_t timeout_ms) {
    brim_rt_internal_set_error(NULL);
    wchar_t *w_url = wide(url->data, url->len);
    URL_COMPONENTS parts;
    memset(&parts, 0, sizeof parts);
    parts.dwStructSize = sizeof parts;
    wchar_t host[256];
    wchar_t path[8192];
    parts.lpszHostName = host;
    parts.dwHostNameLength = 256;
    parts.lpszUrlPath = path;
    parts.dwUrlPathLength = 8192;
    wchar_t extra[8192];
    parts.lpszExtraInfo = extra;
    parts.dwExtraInfoLength = 8192;
    BOOL cracked = WinHttpCrackUrl(w_url, 0, 0, &parts);
    free(w_url);
    if (!cracked || parts.dwHostNameLength == 0 ||
        (parts.nScheme != INTERNET_SCHEME_HTTPS && parts.nScheme != INTERNET_SCHEME_HTTP)) {
        return fail("invalid URL");
    }
    /* The path and query together. */
    wchar_t target[16384];
    swprintf(target, 16384, L"%ls%ls", parts.dwUrlPathLength ? path : L"/",
             parts.dwExtraInfoLength ? extra : L"");

    HINTERNET session = WinHttpOpen(L"brim", WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                                    WINHTTP_NO_PROXY_NAME, WINHTTP_NO_PROXY_BYPASS, 0);
    if (session == NULL) {
        return fail(reason(GetLastError()));
    }
    int t = timeout_ms > INT32_MAX ? INT32_MAX : (int)timeout_ms;
    WinHttpSetTimeouts(session, t, t, t, t);
    HINTERNET conn = WinHttpConnect(session, host, parts.nPort, 0);
    wchar_t *w_method = wide(method->data, method->len);
    HINTERNET req = conn == NULL
        ? NULL
        : WinHttpOpenRequest(conn, w_method, target, NULL, WINHTTP_NO_REFERER,
                             WINHTTP_DEFAULT_ACCEPT_TYPES,
                             parts.nScheme == INTERNET_SCHEME_HTTPS ? WINHTTP_FLAG_SECURE : 0);
    free(w_method);
    BrimStr *result = NULL;
    Bytes head = {0};
    Bytes content = {0};
    DWORD status = 0;
    if (req == NULL) {
        result = fail(reason(GetLastError()));
        goto done;
    }
    DWORD never = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
    WinHttpSetOption(req, WINHTTP_OPTION_REDIRECT_POLICY, &never, sizeof never);
    if (headers->len > 0) {
        wchar_t *w_headers = wide(headers->data, headers->len);
        WinHttpAddRequestHeaders(req, w_headers, (DWORD)-1L, WINHTTP_ADDREQ_FLAG_ADD);
        free(w_headers);
    }
    if (!WinHttpSendRequest(req, WINHTTP_NO_ADDITIONAL_HEADERS, 0,
                            body->len > 0 ? (LPVOID)body->data : WINHTTP_NO_REQUEST_DATA,
                            (DWORD)body->len, (DWORD)body->len, 0) ||
        !WinHttpReceiveResponse(req, NULL)) {
        result = fail(reason(GetLastError()));
        goto done;
    }
    DWORD size = sizeof status;
    WinHttpQueryHeaders(req, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                        WINHTTP_HEADER_NAME_BY_INDEX, &status, &size, WINHTTP_NO_HEADER_INDEX);
    /* The header lines, without the status line and the blank line. */
    DWORD raw_size = 0;
    WinHttpQueryHeaders(req, WINHTTP_QUERY_RAW_HEADERS_CRLF, WINHTTP_HEADER_NAME_BY_INDEX, NULL,
                        &raw_size, WINHTTP_NO_HEADER_INDEX);
    if (raw_size > 0) {
        wchar_t *raw = malloc(raw_size + sizeof(wchar_t));
        if (raw == NULL) {
            abort();
        }
        if (WinHttpQueryHeaders(req, WINHTTP_QUERY_RAW_HEADERS_CRLF, WINHTTP_HEADER_NAME_BY_INDEX,
                                raw, &raw_size, WINHTTP_NO_HEADER_INDEX)) {
            int n = WideCharToMultiByte(CP_UTF8, 0, raw, (int)(raw_size / sizeof(wchar_t)), NULL,
                                        0, NULL, NULL);
            char *text = malloc((size_t)n + 1);
            if (text == NULL) {
                abort();
            }
            WideCharToMultiByte(CP_UTF8, 0, raw, (int)(raw_size / sizeof(wchar_t)), text, n,
                                NULL, NULL);
            text[n] = 0;
            char *line = strstr(text, "\r\n");
            line = line == NULL ? text + n : line + 2;
            while (*line != 0) {
                char *end = strstr(line, "\r\n");
                size_t len = end == NULL ? strlen(line) : (size_t)(end - line);
                if (len > 0) {
                    bytes_add(&head, line, len);
                    bytes_add(&head, "\r\n", 2);
                }
                line += len + (end == NULL ? 0 : 2);
            }
            free(text);
        }
        free(raw);
    }
    if (strcmp_len(method, "HEAD") != 0) {
        static char chunk[65536];
        for (;;) {
            DWORD got = 0;
            if (!WinHttpReadData(req, chunk, sizeof chunk, &got)) {
                result = fail(reason(GetLastError()));
                goto done;
            }
            if (got == 0) {
                break;
            }
            bytes_add(&content, chunk, got);
        }
    }
    if (!brim_rt_internal_valid_utf8(content.data, (int64_t)content.len)) {
        result = fail("the response is not valid UTF-8");
        goto done;
    }
    {
        Bytes out = {0};
        char status_text[32];
        int n = snprintf(status_text, sizeof status_text, "%lu\r\n", (unsigned long)status);
        bytes_add(&out, status_text, (size_t)n);
        bytes_add(&out, head.data, head.len);
        bytes_add(&out, "\r\n", 2);
        bytes_add(&out, content.data, content.len);
        result = brim_rt_internal_str(out.data, (int64_t)out.len);
        free(out.data);
    }
done:
    free(head.data);
    free(content.data);
    if (req != NULL) {
        WinHttpCloseHandle(req);
    }
    if (conn != NULL) {
        WinHttpCloseHandle(conn);
    }
    WinHttpCloseHandle(session);
    return result;
}
#else
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
        curl_easy_setopt(curl, CURLOPT_POSTFIELDS, (const char *)body->data);
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
#endif
