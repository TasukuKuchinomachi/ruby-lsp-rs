class Vault
  def open
    secret
    self.secret
  end

  def secret
    "secret"
  end
end

Vault.new.secret
Vault.new.open
