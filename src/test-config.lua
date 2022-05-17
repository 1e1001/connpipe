route("", 25565, "25565", function(address, port, subport)
	return {"localhost", 8000};
end);
route("", 25565, "", function(address, port, subport)
	return {"localhost", 8000};
end);
