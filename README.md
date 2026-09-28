Toss
====

Drop a file on it.
Toss figures out what to do.

``` text
archive.7z → extract
folder/    → compress
image.jpg  → view
movie.mkv  → play
```

There is no home screen and there is no launcher. You hand Toss a file
or a directory, and it performs the safest useful default action.

On Windows you can drag a file or directory onto `toss.exe`; that behaves
exactly like passing the path on the command line.

``` text
toss photo.jpg
toss movie.mkv
toss archive.7z
toss Documents/
```

Advanced behaviour uses verbs instead of guessing:

``` text
toss extract archive.7z
toss pack folder/
toss info archive.7z
```

## Why

On a machine that is missing the software you normally rely on, Toss
should be able to finish common emergency file-handling tasks by itself.
It aims to solve the common 80% reliably rather than the advanced 100%
badly.

It is deliberately not a replacement for a specialised viewer, player,
or archive manager.

## Safety

Toss never silently does anything destructive. The default preference is:

``` text
read-only  →  create new output  →  modify original  →  delete
```

So an archive extracts into a new directory, and a directory compresses
into a new archive. Existing data is never overwritten without you
asking for it explicitly.

## Status

Early development (v0.1 scope). The repository scaffold and build
pipeline are in place; handlers are not implemented yet, so this README
does not advertise functionality that does not exist.

Planned for v0.1:

``` text
Archive   .zip .7z .rar      → extract
Directory folder/             → compress
Image     .jpg .png .bmp ...  → view
Media     .mp4 .mkv .mp3 ...  → play
```

Anything Toss cannot classify still gets defined behaviour rather than
an error: it reports the file's facts instead of refusing to help.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  http://opensource.org/licenses/MIT)

at your option.
