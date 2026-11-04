class Logger
  def write(message, level: :info)
    yield message
  end

  def info(...)
    write(...)
  end

  def tagged(tag, ...)
    write(...)
  end
end
