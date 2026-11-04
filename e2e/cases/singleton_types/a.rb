registry = Registry.new
registry.register(Plugin)
registry.register(String)

plugin_class = Plugin
plugin_class.load.no_such_method
