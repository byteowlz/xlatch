var ExtensionPreprocessingJS = new function() {
    this.run = function(arguments) {
        arguments.completionFunction({
            url: document.location.href,
            title: document.title.slice(0, 500),
            text: (document.body ? document.body.innerText : "").slice(0, 24000)
        });
    };
};
