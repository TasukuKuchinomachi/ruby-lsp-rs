channel = Pipe.new.channel
channel.write(channel.read)
channel.no_such_method

# @type var both: _ByteSource & _ByteSink
both = Stream.new
