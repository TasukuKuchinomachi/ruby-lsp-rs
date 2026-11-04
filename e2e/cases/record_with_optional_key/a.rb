form = Form.new
form.submit({ name: "x" })
form.submit({ name: "x", age: 1 })
form.submit({ age: 1 })
form.submit({ name: "x", age: "one" })
