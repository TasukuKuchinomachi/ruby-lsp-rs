# frozen_string_literal: true

require "rbconfig"

result = {
  ruby_version: RUBY_VERSION,
  rubylibdir: RbConfig::CONFIG["rubylibdir"],
  rubylibprefix: RbConfig::CONFIG["rubylibprefix"],
  load_path: [],
  bundle_path: nil,
  locked_gems: [],
  initial_excluded_gems: [],
  default_gems_in_bundle: [],
  direct_dependencies: [],
  all_dependencies: [],
  dependencies: [],
  rbs_core_path: nil,
  rubocop_version: nil,
  errors: [],
}

workspace_path = ARGV.fetch(0, Dir.pwd)

begin
  require "bundler"
  Bundler.ui.level = :silent if Bundler.respond_to?(:ui) && Bundler.ui.respond_to?(:level=)

  begin
    Bundler.with_original_env { Bundler.default_gemfile }
    Bundler.setup
    has_bundle = true
  rescue Bundler::GemfileNotFound, Bundler::GitError
    has_bundle = false
  rescue StandardError, LoadError => error
    has_bundle = false
    result[:errors] << "Bundler setup failed: #{error.class}: #{error.message}"
  end

  if has_bundle
    result[:bundle_path] = Bundler.settings["path"]
    locked_gems = Bundler.locked_gems&.specs || []

    locked_gems.each do |lazy_spec|
      spec = Gem::Specification.find_by_name(lazy_spec.name)
      result[:locked_gems] << {
        name: spec.name,
        version: spec.version.to_s,
        full_gem_path: spec.full_gem_path,
        require_paths: spec.require_paths,
      }
    rescue Gem::MissingSpecError
      next
    end

    Dir.glob(File.join(RbConfig::CONFIG["rubylibdir"], "*")).each do |default_path|
      short_name = File.basename(default_path).delete_suffix(".rb")
      in_bundle = locked_gems.any? do |locked_spec|
        locked_spec.name == short_name &&
          !Gem::Specification.find_by_name(short_name).full_gem_path.start_with?(RbConfig::CONFIG["rubylibprefix"])
      rescue Gem::MissingSpecError
        true
      end
      result[:default_gems_in_bundle] << short_name if in_bundle
    end

    begin
      excluded, others = Bundler.definition.dependencies.partition do |dependency|
        dependency.groups == [:development]
      end

      this_gem = Bundler.definition.dependencies.find do |dependency|
        dependency.to_spec&.full_gem_path == workspace_path
      rescue Gem::MissingSpecError
        false
      end

      others.concat(this_gem.to_spec.dependencies) if this_gem
      others.concat(
        others.filter_map do |dependency|
          dependency.to_spec&.dependencies
        rescue Gem::MissingSpecError
          nil
        end.flatten,
      )
      others.uniq!
      others.map!(&:name)

      transitive_excluded = excluded.each_with_object([]) do |dependency, accumulator|
        next unless dependency.runtime?

        spec = dependency.to_spec
        next unless spec

        spec.dependencies.each do |transitive_dependency|
          next if others.include?(transitive_dependency.name)

          accumulator << transitive_dependency
        end
      rescue Gem::MissingSpecError
        next
      end

      excluded.concat(transitive_excluded)
      excluded.uniq!
      result[:initial_excluded_gems] = excluded.map(&:name)
    rescue StandardError => error
      result[:errors] << "Failed to compute excluded gems: #{error.message}"
    end

    direct = Bundler.locked_gems&.dependencies&.keys || []
    gemspec_dependencies = (Bundler.locked_gems&.sources || [])
      .grep(Bundler::Source::Gemspec)
      .flat_map { |source| source.gemspec&.dependencies&.map(&:name) || [] }
    result[:direct_dependencies] = direct + gemspec_dependencies
    result[:all_dependencies] = locked_gems.map(&:name)

    begin
      definition = Bundler.definition
      dependency_keys = definition.locked_deps.keys
      result[:dependencies] = definition.specs.map do |spec|
        {
          name: spec.name,
          version: spec.version.to_s,
          path: spec.full_gem_path,
          dependency: dependency_keys.include?(spec.name),
        }
      end
    rescue StandardError
      result[:dependencies] = []
    end
  end
rescue LoadError => error
  result[:errors] << "Bundler is not available: #{error.message}"
end

result[:load_path] = $LOAD_PATH.map(&:to_s)

begin
  rbs_spec = Gem::Specification.find_by_name("rbs")
  result[:rbs_core_path] = File.join(rbs_spec.full_gem_path, "core")
rescue StandardError, LoadError
  candidates = Dir.glob(File.join(Gem.default_dir, "gems", "rbs-*", "core"))
  result[:rbs_core_path] = candidates.max_by { |path| Gem::Version.new(path[/rbs-([\d.]+)/, 1] || "0") }
end

begin
  result[:rubocop_version] = Gem::Specification.find_by_name("rubocop").version.to_s
rescue StandardError, LoadError
  nil
end

begin
  result[:gem_stubs] = Gem::Specification.stubs.map do |stub|
    { name: stub.name, version: stub.version.to_s, full_gem_path: stub.full_gem_path }
  end
  result[:default_gem_stubs] = Gem::Specification.default_stubs.filter_map do |stub|
    spec = stub.to_spec
    next unless spec

    { name: spec.name, version: spec.version.to_s, require_paths: spec.require_paths, files: spec.files }
  end
rescue StandardError, LoadError => error
  result[:errors] << "Failed to list gem specifications: #{error.message}"
end

# Requiring json before Bundler.setup activates the newest installed json and makes the setup fail when the bundle
# locks another version
require "json"
$stdout.write(JSON.generate(result))
