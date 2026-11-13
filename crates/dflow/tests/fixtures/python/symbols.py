import pkg.helper as helper
from .tools import notify as send

@decorate(factory())
async def process(item=default_call()):
    def inner():
        return helper.run(item)
    class Local:
        def method(self):
            return send(inner())
    return Local()
