# frozen_string_literal: true

# Editors launched from the Dock or Finder pass no LANG, which makes the default US-ASCII and breaks on non-ASCII
# sources and RuboCop configurations
Encoding.default_external = Encoding::UTF_8
Encoding.default_internal = nil

protocol_output = $stdout.dup
protocol_output.set_encoding(Encoding::UTF_8)
protocol_output.sync = true
$stdout.reopen($stderr)
protocol_input = $stdin.dup
protocol_input.set_encoding(Encoding::UTF_8)
$stdin.reopen(File::NULL)

workspace_path = ARGV.fetch(0, Dir.pwd)
Dir.chdir(workspace_path)

startup_errors = []
bundle_ready = false

begin
  require "bundler"
  Bundler.ui.level = :silent if Bundler.respond_to?(:ui) && Bundler.ui.respond_to?(:level=)

  begin
    Bundler.with_original_env { Bundler.default_gemfile }
    Bundler.setup
    bundle_ready = true
  rescue Bundler::GemfileNotFound
    bundle_ready = false
  rescue StandardError, LoadError => error
    startup_errors << "Bundler setup failed: #{error.class}: #{error.message}"
  end
rescue LoadError => error
  startup_errors << "Bundler is not available: #{error.message}"
end

# Requiring json before Bundler.setup activates the newest installed json and makes the setup fail when the bundle
# locks another version
require "json"

rubocop_loaded = false
if bundle_ready
  begin
    require "rubocop"

    begin
      gem("rubocop", ">= 1.4.0")
      RuboCop::LSP.enable if RuboCop.const_defined?(:LSP)
      rubocop_loaded = true
    rescue LoadError
      $stderr.puts "Incompatible RuboCop version. Ruby LSP requires >= 1.4.0"
    end
  rescue LoadError
    rubocop_loaded = false
  end
end

syntax_tree_loaded = false
if bundle_ready
  begin
    require "syntax_tree"
    require "syntax_tree/cli"
    syntax_tree_loaded = true
  rescue LoadError
    syntax_tree_loaded = false
  end
end

module RubyLspFormatterWorker
  class RequestError < StandardError; end
end

if rubocop_loaded
  module RubyLspFormatterWorker
    class InternalRuboCopError < StandardError
      MESSAGE = <<~EOS
        An internal error occurred %s.
        Updating to a newer version of RuboCop may solve this.
        For more details, run RuboCop on the command line.
      EOS

      def initialize(rubocop_error)
        message = case rubocop_error
        when ::RuboCop::ErrorWithAnalyzedFileLocation
          format(MESSAGE, "for the #{rubocop_error.cop.name} cop")
        when StandardError
          format(MESSAGE, rubocop_error.message)
        end
        super(message)
      end
    end

    class RuboCopRunner < ::RuboCop::Runner
      class ConfigurationError < StandardError; end

      DEFAULT_ARGS = [
        "--stderr",
        "--force-exclusion",
        "--format",
        "RuboCop::Formatter::BaseFormatter",
      ]

      attr_reader :offenses
      attr_reader :config_for_working_directory

      begin
        ::RuboCop::Options.new.parse(["--raise-cop-error"])
        DEFAULT_ARGS << "--raise-cop-error"
      rescue OptionParser::InvalidOption
        nil
      end
      DEFAULT_ARGS.freeze

      def initialize(*args)
        @options = {}
        @offenses = []
        @errors = []
        @warnings = []

        args += DEFAULT_ARGS
        rubocop_options = ::RuboCop::Options.new.parse(args).first

        config_store = ::RuboCop::ConfigStore.new
        config_store.options_config = rubocop_options[:config] if rubocop_options[:config]
        @config_for_working_directory = config_store.for_pwd

        super(rubocop_options, config_store)
      end

      def run(path, contents)
        @errors = []
        @warnings = []
        @offenses = []
        @options[:stdin] = contents

        super([path])

        raise Interrupt if aborting?
      rescue ::RuboCop::Runner::InfiniteCorrectionLoop => error
        raise RequestError, error.message
      rescue ::RuboCop::ValidationError => error
        raise ConfigurationError, error.message
      rescue StandardError => error
        internal_error = InternalRuboCopError.new(error)
        internal_error.set_backtrace(error.backtrace)
        raise internal_error
      end

      def formatted_source
        @options[:stdin]
      end

      class << self
        def find_cop_by_name(cop_name)
          cop_registry[cop_name]&.first
        end

        private

        def cop_registry
          @cop_registry ||= ::RuboCop::Cop::Registry.global.to_h
        end
      end

      private

      def file_finished(_file, offenses)
        @offenses = offenses
      end
    end

    class RuboCopDiagnostic
      DIAGNOSTIC_SEVERITY_ERROR = 1
      DIAGNOSTIC_SEVERITY_WARNING = 2
      DIAGNOSTIC_SEVERITY_INFORMATION = 3
      DIAGNOSTIC_SEVERITY_HINT = 4

      RUBOCOP_TO_LSP_SEVERITY = {
        info: DIAGNOSTIC_SEVERITY_HINT,
        refactor: DIAGNOSTIC_SEVERITY_INFORMATION,
        convention: DIAGNOSTIC_SEVERITY_INFORMATION,
        warning: DIAGNOSTIC_SEVERITY_WARNING,
        error: DIAGNOSTIC_SEVERITY_ERROR,
        fatal: DIAGNOSTIC_SEVERITY_ERROR,
      }.freeze

      SELF_RESOLVING_DISABLE_COPS = ["Layout/EmptyComment"].freeze

      SURROGATE_PAIR_START = 0xFFFF

      ENHANCED_DOC_URL = begin
        gem("rubocop", ">= 1.64.0")
        true
      rescue LoadError
        false
      end

      def initialize(encoding, offense, uri)
        @encoding = encoding
        @offense = offense
        @uri = uri
      end

      def to_lsp_code_actions
        code_actions = []

        code_actions << autocorrect_action if correctable?
        code_actions << disable_line_action unless SELF_RESOLVING_DISABLE_COPS.include?(@offense.cop_name)

        code_actions
      end

      def to_lsp_diagnostic(config)
        highlighted = @offense.highlighted_area
        diagnostic = {
          range: range(@offense.line - 1, highlighted.begin_pos, @offense.line - 1, highlighted.end_pos),
          severity: severity,
          code: @offense.cop_name,
        }
        description = code_description(config)
        diagnostic[:codeDescription] = description if description
        diagnostic[:source] = "RuboCop"
        diagnostic[:message] = message
        diagnostic[:data] = {
          correctable: correctable?,
          code_actions: to_lsp_code_actions,
        }
        diagnostic.delete(:severity) if diagnostic[:severity].nil?
        diagnostic
      end

      private

      def range(start_line, start_character, end_line, end_character)
        {
          start: { line: start_line, character: start_character },
          end: { line: end_line, character: end_character },
        }
      end

      def message
        message  = @offense.message
        message += "\n\nThis offense is not auto-correctable.\n" unless correctable?
        message
      end

      def severity
        RUBOCOP_TO_LSP_SEVERITY[@offense.severity.name]
      end

      def code_description(config)
        cop = RuboCopRunner.find_cop_by_name(@offense.cop_name)
        return unless cop

        doc_url = if ENHANCED_DOC_URL
          cop.documentation_url(config)
        else
          cop.documentation_url
        end
        { href: doc_url } if doc_url
      end

      def autocorrect_action
        {
          title: "Autocorrect #{@offense.cop_name}",
          kind: "quickfix",
          isPreferred: true,
          edit: {
            documentChanges: [
              {
                textDocument: { uri: @uri, version: nil },
                edits: correctable? ? offense_replacements : [],
              },
            ],
          },
        }
      end

      def offense_replacements
        @offense.corrector.as_replacements.map do |replacement_range, replacement|
          {
            range: range(
              replacement_range.line - 1,
              replacement_range.column,
              replacement_range.last_line - 1,
              replacement_range.last_column,
            ),
            newText: replacement,
          }
        end
      end

      def disable_line_action
        {
          title: "Disable #{@offense.cop_name} for this line",
          kind: "quickfix",
          edit: {
            documentChanges: [
              {
                textDocument: { uri: @uri, version: nil },
                edits: line_disable_comment,
              },
            ],
          },
        }
      end

      def line_disable_comment
        new_text = if @offense.source_line.include?(" # rubocop:disable ")
          ",#{@offense.cop_name}"
        else
          " # rubocop:disable #{@offense.cop_name}"
        end

        end_of_line = length_of_line(@offense.source_line)

        [
          {
            range: range(@offense.line - 1, end_of_line, @offense.line - 1, end_of_line),
            newText: new_text,
          },
        ]
      end

      def length_of_line(line)
        if @encoding == "utf-16"
          line_length = 0
          line.codepoints.each do |codepoint|
            line_length += 1
            line_length += 1 if codepoint > SURROGATE_PAIR_START
          end
          line_length
        else
          line.length
        end
      end

      def correctable?
        !@offense.corrector.nil?
      end
    end

    class RuboCopFormatter
      def initialize
        @diagnostic_runner = RuboCopRunner.new
        @format_runner = RuboCopRunner.new("-a")
      end

      def run_formatting(path, source)
        @format_runner.run(path, source)
        @format_runner.formatted_source
      end

      def run_range_formatting(_path, _source, _base_indentation)
        nil
      end

      def run_diagnostic(path, uri, source, encoding)
        @diagnostic_runner.run(path, source)

        @diagnostic_runner.offenses.map do |offense|
          RuboCopDiagnostic.new(encoding, offense, uri).to_lsp_diagnostic(@diagnostic_runner.config_for_working_directory)
        end
      end
    end
  end
end

if syntax_tree_loaded
  module RubyLspFormatterWorker
    class SyntaxTreeFormatter
      def initialize
        @options = begin
          options = SyntaxTree::CLI::Options.new
          options.parse(SyntaxTree::CLI::ConfigFile.new.arguments)
          options
        end
      end

      def run_formatting(path, source)
        return if ignored?(path)

        SyntaxTree.format(source, @options.print_width, options: @options.formatter_options)
      end

      def run_range_formatting(path, source, base_indentation)
        return if ignored?(path)

        SyntaxTree.format(source, @options.print_width, base_indentation, options: @options.formatter_options)
      end

      def run_diagnostic(_path, _uri, _source, _encoding)
        nil
      end

      private

      def ignored?(path)
        path && @options.ignore_files.any? { |pattern| File.fnmatch?("*/#{pattern}", path) }
      end
    end
  end
end

supported_formatters = {}
show_messages = []

create_rubocop_formatter = lambda do
  supported_formatters["rubocop_internal"] = RubyLspFormatterWorker::RuboCopFormatter.new
  nil
rescue ::RuboCop::Error => error
  supported_formatters.delete("rubocop_internal")
  "RuboCop configuration error: #{error.message}. Formatting will not be available."
end

if rubocop_loaded
  configuration_error = create_rubocop_formatter.call
  show_messages << configuration_error if configuration_error
end
supported_formatters["syntax_tree"] = RubyLspFormatterWorker::SyntaxTreeFormatter.new if syntax_tree_loaded

write_message = lambda do |message|
  protocol_output.write(JSON.generate(message))
  protocol_output.write("\n")
end

write_message.call({
  ready: true,
  rubocop_loaded: rubocop_loaded,
  formatters: supported_formatters.keys,
  show_messages: show_messages,
  errors: startup_errors,
})

handle_request = lambda do |request|
  case request["method"]
  when "formatting"
    formatter = supported_formatters[request["formatter"]]
    formatter&.run_formatting(request["path"], request["source"])
  when "range_formatting"
    formatter = supported_formatters[request["formatter"]]
    formatter&.run_range_formatting(request["path"], request["source"], request["base_indentation"])
  when "diagnostic"
    request["linters"].flat_map do |linter|
      formatter = supported_formatters[linter]
      next [] unless formatter

      formatter.run_diagnostic(request["path"], request["uri"], request["source"], request["encoding"]) || []
    end
  when "reload_rubocop"
    next { rubocop_loaded: false, formatters: supported_formatters.keys } unless rubocop_loaded

    configuration_error = create_rubocop_formatter.call
    { rubocop_loaded: true, formatters: supported_formatters.keys, show_message: configuration_error }
  else
    raise RubyLspFormatterWorker::RequestError, "Unknown method #{request["method"]}"
  end
end

while (line = protocol_input.gets)
  next if line.strip.empty?

  request = JSON.parse(line)
  response = begin
    { id: request["id"], result: handle_request.call(request) }
  rescue StandardError, LoadError, Interrupt => error
    {
      id: request["id"],
      error: { class: error.class.name, message: error.message, full_message: error.full_message(highlight: false) },
    }
  end

  begin
    write_message.call(response)
  rescue JSON::GeneratorError => error
    write_message.call({ id: request["id"], error: { class: error.class.name, message: error.message, full_message: error.message } })
  end
end
