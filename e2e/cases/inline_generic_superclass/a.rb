class Names < Array #[String]
  #: () -> String
  def first_name
    first || ""
  end
end

Names.new.first_name.no_such_method
Names.new.push(1)
