module Garage
  class Car
    def honk
      "beep"
    end
  end
end

class Mechanic
  def inspect_cars(cars, spare)
    total = 0
    cars.each do |car|
      car.honk
      total += 1
    end
    found = cars.first
    vehicle = spare ? spare : Garage::Truck.new
    vehicle
    found
    total
  end
end
