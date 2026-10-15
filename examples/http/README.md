# notes: a JSON API in Brim

A small service for notes, kept in memory, built with `std.http`,
`std.json`, `std.log` and `std.time`.

```sh
brim run notes.brim                  # http://127.0.0.1:8080
curl -X POST localhost:8080/notes -d '{"text": "buy milk"}'
curl localhost:8080/notes
curl localhost:8080/notes/1
curl -X DELETE localhost:8080/notes/1
```

`PORT` and `HOST` set where it listens. As a container:

```sh
docker build -t notes .
docker run -p 8080:8080 notes
```

The image holds only the native executable (about 100 KB) on Debian slim,
and runs as `nobody`.
