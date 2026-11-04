class Inventory
  def initialize
  end

  def raw
    nil
  end

  def raw_count
    nil
  end

  def count
    0
  end

  def take(number)
  end

  def take_options(**options)
  end

  def receivers(value)
    value.upcase(1, 2)
    value.strip
    raw.first.second.third
    raw.first.second.third
    raw_count.each { |item| item.name }
    no_such_method.more
    UnknownConstant.more
    (_ = count).succ
    count.succ
  end

  def other_method(value)
    value.strip
  end

  def unions(count)
    first = count || raw
    second = raw || 0
    third = raw && count
    range = 1..raw
    fourth = raw || raw_count
    fifth = count && raw
    take(raw || 1) if raw || count
    "#{raw || 1}"
  end

  def optional_call(inventory)
    untyped_result = inventory&.raw
    nil
  end

  def branches(flag, number)
    ternary = flag ? raw : 1
    without_else = if flag then raw end
    cases = case number
            when 1 then raw
            else "many"
            end
    all_untyped = flag ? raw : raw_count
    explicit = flag ? (_ = count) : 1
    take(flag ? raw : 1)
    nil
  end

  def statements(flag)
    if flag
      raw
    else
      1
    end
    nil
  end

  def rescues
    first = begin
      count
    rescue
      raw
    end
    begin
      raw
    rescue
      1
    end
  end

  def hashes(options, record)
    value = { a: 1, b: raw }
    key = { raw => 1, b: 2 }
    splat = { a: 1, **options }
    record_splat = { a: 1, **record }
    only_untyped = { a: raw }
    take_options(a: 1, b: raw)
    nil
  end

  def declared
    # @type var total: Integer
    total = raw
    total = count
    total = _ = raw
    nil
  end
end
