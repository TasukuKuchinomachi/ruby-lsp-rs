module Outer
  module Inner
    class Item
      def initialize(member)
        @member = member
      end

      def member
        @member
      end

      def describe
        case member
        when String
          member.upcase
        when Integer
          member + 1
        end
      end

      def replace(member)
        @member = member
      end
    end
  end
end

item = Outer::Inner::Item.new("a")
item.member.foo
item.replace(:symbol)
item.replace(1)
