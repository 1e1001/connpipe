# connpipe protocol

on connection:

- random data &rarr; subport = `None`
- `magic` `subport-len` `subport` &rarr; subport = `Some(subport)`

`magic` = `89 3d 1d 4e 4e 4f c6 86 2d 1e 10 01 0d 0a 69 0a`

`subport-len` = base 128 Varint in little endian

- `1xxxxxxx` - push bits `xxxxxxx` and read one more byte
- `0xxxxxxx` - push bits `xxxxxxx` and end reading

if more than `usize::BITS` bits are read then cancel connection

if value is ever more than `maximum_name_length` then cancel connection

`subport` = `subport-len` bytes

as of right now it has to always be valid UTF-8.
