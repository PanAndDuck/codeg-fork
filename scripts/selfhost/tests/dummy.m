#import <Cocoa/Cocoa.h>
#include <arpa/inet.h>
#include <sys/socket.h>
#include <unistd.h>

// A real signed macOS app: LaunchServices, Apple Events and TCP are not mocked.
int main(void) {
  @autoreleasepool {
    NSBundle *bundle = [NSBundle mainBundle];
    if ([[NSFileManager defaultManager] fileExistsAtPath:
        [[bundle resourcePath] stringByAppendingPathComponent:@"fail"]]) return 42;
    NSString *port = [bundle objectForInfoDictionaryKey:@"TestPort"];
    int fd = socket(AF_INET, SOCK_STREAM, 0), reuse = 1;
    setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &reuse, sizeof(reuse));
    struct sockaddr_in address = {0};
    address.sin_family = AF_INET;
    address.sin_port = htons([port intValue]);
    address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    if (bind(fd, (struct sockaddr *)&address, sizeof(address)) || listen(fd, 8)) return 43;
    NSApplication *app = [NSApplication sharedApplication];
    [app setActivationPolicy:NSApplicationActivationPolicyProhibited];
    [app run];
    close(fd);
  }
  return 0;
}
