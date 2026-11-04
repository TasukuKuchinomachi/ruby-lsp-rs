class Narrowing
  def run
    if (a, b = pair)
      a.no_such_method
      b.no_such_method
    else
      a.no_such_method
      b.no_such_method
    end

    while (x, y = numbers)
      x.no_such_method
      y.no_such_method
    end

    (c, d = pair) or return
    c.no_such_method
    d.no_such_method

    if (e, *f, g = pair)
      e.no_such_method
      f.no_such_method
      g.no_such_method
    end

    if (h, (i, j) = pair)
      h.no_such_method
      i.no_such_method
      j.no_such_method
    end

    if (k, l = anything)
      k.no_such_method
      l.no_such_method
    end

    if (m, n = number)
      m.no_such_method
      n.no_such_method
    else
      m.no_such_method
      n.no_such_method
    end

    unless (@left, o = pair)
      o.no_such_method
    end

    if (prefix, items = completion)
      items.map { _1.no_such_method }
      prefix.no_such_method
    end
  end
end
