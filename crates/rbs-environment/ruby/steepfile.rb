# Evaluates the Steepfile given as the first argument with a DSL following Steep 2.0.0 `Steep::Project::DSL`, without
# loading Steep, and prints the targets as JSON. Diagnostic configurations are kept as the template they start from and
# the codes set over it, since only the names of Steep's diagnostic classes are known here

require "json"
require "pathname"

module Steep
  module Diagnostic
    class Code
      attr_reader :name

      def initialize(name)
        @name = name
      end

      def hash
        name.hash
      end

      def eql?(other)
        other.is_a?(Code) && other.name == name
      end

      alias == eql?
    end

    class Configuration < Hash
      attr_accessor :template

      def self.from_template(template)
        new.tap {|configuration| configuration.template = template }
      end

      def self.coerce(hash)
        from_template(hash.is_a?(Configuration) ? hash.template : nil).merge!(hash)
      end

      def merge!(other)
        if other.is_a?(Configuration)
          self.template = other.template
          clear
        end
        other.each {|code, severity| self[code] = severity }
        self
      end

      alias update merge!

      def merge(other)
        dup.merge!(other)
      end

      def dup
        Configuration.from_template(template).tap do |copy|
          each {|code, severity| copy[code] = severity }
        end
      end

      def to_json_value
        {
          "template" => template&.to_s,
          "severities" => each.with_object({}) do |(code, severity), hash|
            name = code.respond_to?(:name) ? code.name : code.to_s
            hash[name] = severity&.to_s
          end
        }
      end
    end

    module Ruby
      %i[default strict lenient silent all_error].each do |template|
        define_singleton_method(template) { Configuration.from_template(template) }
      end

      def self.const_missing(name)
        const_set(name, Code.new("Ruby::#{name}"))
      end
    end
  end

  class SteepfileDSL
    module WithPattern
      def check(*patterns, inline: false)
        @value["check"].concat(patterns.map(&:to_s))
      end

      def ignore(*patterns, inline: false)
        @value["ignore"].concat(patterns.map(&:to_s))
      end

      def signature(*patterns)
        @value["signature"].concat(patterns.map(&:to_s))
      end

      def ignore_signature(*patterns)
        @value["ignore_signature"].concat(patterns.map(&:to_s))
      end
    end

    module LibraryOptions
      def library(*names)
        @value["library_configured"] = true
        @value["libraries"].concat(names.map(&:to_s))
      end

      def repo_path(*)
        @value["library_configured"] = true
      end

      def stdlib_path(core_root:, stdlib_root:)
      end

      def collection_config(path)
        @value["library_configured"] = true
        @value["collection_config"] = path.to_s
      end

      def disable_collection
        @value["library_configured"] = true
        @value["collection_config"] = false
      end
    end

    def self.patterns
      { "check" => [], "ignore" => [], "signature" => [], "ignore_signature" => [] }
    end

    class GroupDSL
      include WithPattern

      attr_reader :value

      def initialize(name, target)
        @target = target
        @configuration = nil
        @value = SteepfileDSL.patterns.merge("name" => name.to_s)
      end

      def configure_code_diagnostics(configuration = nil)
        if block_given?
          if @configuration
            @configuration.merge!(configuration) if configuration
          else
            @configuration = Diagnostic::Configuration.coerce(configuration || @target.code_diagnostics)
          end
          yield @configuration
        else
          @configuration = configuration && Diagnostic::Configuration.coerce(configuration)
        end
      end

      def to_json_value
        value.merge("code_diagnostics" => @configuration&.to_json_value)
      end
    end

    class TargetDSL
      include WithPattern
      include LibraryOptions

      attr_reader :value, :code_diagnostics

      def initialize(name)
        @code_diagnostics = Diagnostic::Ruby.default
        @groups = []
        @value = SteepfileDSL.patterns.merge(
          "name" => name.to_s,
          "libraries" => [],
          "library_configured" => false,
          "collection_config" => nil,
          "unreferenced" => false,
          "implicitly_returns_nil" => false
        )
      end

      def unreferenced!(value = true)
        @value["unreferenced"] = value ? true : false
      end

      def implicitly_returns_nil!(value = true)
        @value["implicitly_returns_nil"] = value ? true : false
      end

      def configure_code_diagnostics(configuration = nil)
        @code_diagnostics.merge!(configuration) if configuration
        yield @code_diagnostics if block_given?
      end

      def group(name, &block)
        group = GroupDSL.new(name, self)
        group.instance_exec(&block) if block
        @groups << group
      end

      def to_json_value
        value.merge(
          "code_diagnostics" => @code_diagnostics.to_json_value,
          "groups" => @groups.map(&:to_json_value)
        )
      end
    end

    include LibraryOptions

    attr_reader :value

    def initialize
      @targets = []
      @value = { "libraries" => [], "library_configured" => false, "collection_config" => nil }
    end

    def target(name, &block)
      target = TargetDSL.new(name)
      target.instance_exec(&block) if block
      @targets << target
    end

    def to_json_value
      value.merge("targets" => @targets.map(&:to_json_value))
    end
  end
end

steepfile = Pathname(ARGV.fetch(0))
dsl = Steep::SteepfileDSL.new
Dir.chdir(steepfile.dirname) do
  dsl.instance_eval(steepfile.read, steepfile.to_s)
end
print JSON.generate(dsl.to_json_value)
