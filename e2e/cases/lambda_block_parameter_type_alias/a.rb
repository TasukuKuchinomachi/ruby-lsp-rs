required = -> (&blk) {
  # @type var blk: callback
  blk.call(1)
}

optional = -> (&blk) {
  # @type var blk: callback?
  blk&.call(1)
}

expanded = -> (&blk) {
  # @type var blk: optional_callback
  blk&.call(1)
}

inline = -> (&blk) {
  # @type var blk: ^(Integer) -> void
  blk.call(1)
}

not_proc = -> (&blk) {
  # @type var blk: not_proc
  blk + 1
}
