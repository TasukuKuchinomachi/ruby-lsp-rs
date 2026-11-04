builder = HtmlBuilder.new
builder.with_name("x").render
builder.copy.render
builder.kind.no_such_method

Builder.new.with_name("x").render
